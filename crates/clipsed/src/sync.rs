//! Driving one sync session to completion.
//!
//! Strictly alternating turns, dialler first — see `docs/sync-protocol.md` §5
//! for why a symmetric exchange deadlocks once both summaries outgrow the QUIC
//! flow-control window.
//!
//! # What this does not do yet
//!
//! Blob payloads are not fetched. A clip whose payload spilled to the blob
//! store arrives with its row and its digest but without its bytes, so
//! `Clip::is_complete` reports false and the UI shows it as incomplete. The
//! machinery for the transfer exists and is tested on both sides
//! (`clipse_sync::chunk`, `PeerLink::send_blob`); what is missing is the
//! offer/want exchange in the turns below. Inline payloads — which is all text,
//! HTML and RTF, and therefore the overwhelming majority of clips — sync fully.
//!
//! Nothing in the daemon calls this yet: the peer manager that dials, accepts
//! and schedules sessions is the next piece. The protocol itself is exercised
//! by the two-daemon tests at the bottom of this file, which stand up two
//! complete stacks — store, clock, identity, QUIC endpoint — and sync between
//! them.
#![allow(dead_code, reason = "driven by tests until the peer manager lands")]

use std::sync::{Arc, Mutex};

use clipse_core::{Clip, ContentHash, DeviceId, Hlc, HlcClock};
use clipse_net::PeerLink;
use clipse_store::Store;
use clipse_sync::{ClipSummary, LoopGuard, MergeAction, SyncMessage, merge};
use tracing::{debug, warn};

/// Clips per `Summary` message. Small enough that a page fits comfortably
/// inside one flow-control window even before the alternation above kicks in.
const SUMMARY_PAGE: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("link: {0}")]
    Link(#[from] clipse_net::LinkError),

    #[error("store: {0}")]
    Store(#[from] clipse_store::Error),

    #[error("peer speaks protocol {theirs}, we speak {ours}")]
    ProtocolMismatch { theirs: u16, ours: u16 },

    #[error("peer sent {got} when {expected} was expected")]
    Unexpected { expected: &'static str, got: String },

    #[error("peer identified itself as {claimed} but authenticated as {actual}")]
    IdentityMismatch { claimed: DeviceId, actual: DeviceId },

    #[error("background task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// Which side of the alternation we are on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Dialler,
    Responder,
}

pub struct SyncContext {
    pub store: Arc<Store>,
    pub clock: Arc<HlcClock>,
    pub loop_guard: Arc<Mutex<LoopGuard>>,
    pub label: String,
    pub platform: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub sent: usize,
    pub received: usize,
    pub rejected: usize,
}

/// Exchange histories with one peer, then return.
pub async fn run_session(
    link: &mut PeerLink,
    ctx: &SyncContext,
    role: Role,
) -> Result<SyncOutcome, SyncError> {
    let peer = link.remote_device();
    let peer_max_hlc = match role {
        Role::Dialler => {
            send_hello(link, ctx).await?;
            receive_hello(link, peer).await?
        }
        Role::Responder => {
            let theirs = receive_hello(link, peer).await?;
            send_hello(link, ctx).await?;
            theirs
        }
    };

    let mut outcome = SyncOutcome::default();
    match role {
        Role::Dialler => {
            offer_our_history(link, ctx, peer_max_hlc, &mut outcome).await?;
            take_their_history(link, ctx, peer, &mut outcome).await?;
        }
        Role::Responder => {
            take_their_history(link, ctx, peer, &mut outcome).await?;
            offer_our_history(link, ctx, peer_max_hlc, &mut outcome).await?;
        }
    }

    debug!(peer = %peer.short(), ?outcome, "sync session complete");
    Ok(outcome)
}

async fn send_hello(link: &mut PeerLink, ctx: &SyncContext) -> Result<(), SyncError> {
    let store = Arc::clone(&ctx.store);
    let max_hlc = tokio::task::spawn_blocking(move || store.max_hlc()).await??;

    link.send(&SyncMessage::Hello {
        device: ctx.clock.device(),
        epoch: link.epoch(),
        protocol: clipse_core::PROTOCOL_VERSION,
        max_hlc,
        label: ctx.label.clone(),
        platform: ctx.platform.clone(),
    })
    .await?;
    Ok(())
}

async fn receive_hello(link: &mut PeerLink, peer: DeviceId) -> Result<Option<Hlc>, SyncError> {
    match link.recv().await? {
        SyncMessage::Hello {
            device,
            protocol,
            max_hlc,
            ..
        } => {
            if protocol != clipse_core::PROTOCOL_VERSION {
                return Err(SyncError::ProtocolMismatch {
                    theirs: protocol,
                    ours: clipse_core::PROTOCOL_VERSION,
                });
            }
            // The Noise handshake already proved who this is. A Hello that
            // claims someone else is either a bug or an attempt to have its
            // clips attributed to a different device.
            if device != peer {
                return Err(SyncError::IdentityMismatch {
                    claimed: device,
                    actual: peer,
                });
            }
            Ok(max_hlc)
        }
        other => Err(SyncError::Unexpected {
            expected: "Hello",
            got: variant_name(&other).to_string(),
        }),
    }
}

/// Our turn to talk: summarise, hear what they want, send it.
///
/// The summary covers our **whole** history, not `changes_since(their
/// max_hlc)`. Their `max_hlc` is the newest thing *they* hold; it says nothing
/// about what they have seen from *us*. Two devices that copy something at the
/// same moment produce concurrent HLCs, and whichever one is numerically lower
/// would never be offered — the clip would simply never arrive. A summary
/// entry is a hash, an HLC and three flags, and `Want` filters precisely, so
/// the cost of being correct here is small.
///
/// The incremental version of this needs a *per-peer* cursor — the highest HLC
/// of ours that this peer has acknowledged — persisted across sessions. `Ack`
/// already carries what that cursor would be built from.
async fn offer_our_history(
    link: &mut PeerLink,
    ctx: &SyncContext,
    _peer_max_hlc: Option<Hlc>,
    outcome: &mut SyncOutcome,
) -> Result<(), SyncError> {
    let store = Arc::clone(&ctx.store);
    let clips = tokio::task::spawn_blocking(move || store.changes_since(None, 100_000)).await??;

    let summaries: Vec<ClipSummary> = clips.iter().map(ClipSummary::of).collect();
    if summaries.is_empty() {
        link.send(&SyncMessage::Summary {
            entries: Vec::new(),
            complete: true,
        })
        .await?;
    } else {
        let pages = summaries.chunks(SUMMARY_PAGE);
        let page_count = pages.len();
        for (index, page) in summaries.chunks(SUMMARY_PAGE).enumerate() {
            link.send(&SyncMessage::Summary {
                entries: page.to_vec(),
                complete: index + 1 == page_count,
            })
            .await?;
        }
    }

    let wanted = match link.recv().await? {
        SyncMessage::Want { hashes } => hashes,
        other => {
            return Err(SyncError::Unexpected {
                expected: "Want",
                got: variant_name(&other).to_string(),
            });
        }
    };

    for hash in wanted {
        // Served from the store rather than from the page we just built: the
        // history may have changed since, and the store is the truth.
        let store = Arc::clone(&ctx.store);
        let found =
            tokio::task::spawn_blocking(move || store.by_hash_including_deleted(&hash)).await??;
        match found {
            Some(clip) => {
                link.send(&SyncMessage::Push {
                    clip: Box::new(clip),
                })
                .await?;
                outcome.sent += 1;
            }
            None => warn!("peer asked for a clip we no longer have; skipping"),
        }
    }

    match link.recv().await? {
        SyncMessage::Ack { .. } => Ok(()),
        other => Err(SyncError::Unexpected {
            expected: "Ack",
            got: variant_name(&other).to_string(),
        }),
    }
}

/// Their turn: hear what they have, ask for what we lack, apply it.
async fn take_their_history(
    link: &mut PeerLink,
    ctx: &SyncContext,
    peer: DeviceId,
    outcome: &mut SyncOutcome,
) -> Result<(), SyncError> {
    let mut entries: Vec<ClipSummary> = Vec::new();
    loop {
        match link.recv().await? {
            SyncMessage::Summary {
                entries: page,
                complete,
            } => {
                entries.extend(page);
                if complete {
                    break;
                }
            }
            other => {
                return Err(SyncError::Unexpected {
                    expected: "Summary",
                    got: variant_name(&other).to_string(),
                });
            }
        }
    }

    let mut wanted = Vec::new();
    for entry in &entries {
        let store = Arc::clone(&ctx.store);
        let hash = entry.hash;
        let local =
            tokio::task::spawn_blocking(move || store.by_hash_including_deleted(&hash)).await??;
        let take = match &local {
            None => true,
            Some(existing) => entry.hlc > existing.hlc,
        };
        if take {
            wanted.push(entry.hash);
        }
    }

    let expected = wanted.len();
    link.send(&SyncMessage::Want { hashes: wanted }).await?;

    for _ in 0..expected {
        match link.recv().await? {
            SyncMessage::Push { clip } => {
                if apply(ctx, peer, *clip).await? {
                    outcome.received += 1;
                } else {
                    outcome.rejected += 1;
                }
            }
            other => {
                return Err(SyncError::Unexpected {
                    expected: "Push",
                    got: variant_name(&other).to_string(),
                });
            }
        }
    }

    let store = Arc::clone(&ctx.store);
    let max_hlc = tokio::task::spawn_blocking(move || store.max_hlc()).await??;
    link.send(&SyncMessage::Ack {
        hlc: max_hlc.unwrap_or_else(|| ctx.clock.now()),
    })
    .await?;

    Ok(())
}

/// Merge one incoming clip. Returns false when it was refused.
async fn apply(ctx: &SyncContext, peer: DeviceId, clip: Clip) -> Result<bool, SyncError> {
    let store = Arc::clone(&ctx.store);
    let hash = clip.hash;
    let local =
        tokio::task::spawn_blocking(move || store.by_hash_including_deleted(&hash)).await??;

    let action = merge(local.as_ref(), &clip);
    let hlc = clip.hlc;

    match action {
        MergeAction::Reject => {
            warn!(peer = %peer.short(), "refused a clip that does not hash to its payloads");
            return Ok(false);
        }
        MergeAction::Ignore => return Ok(false),
        MergeAction::Insert => {
            let store = Arc::clone(&ctx.store);
            let to_insert = clip.clone();
            tokio::task::spawn_blocking(move || store.insert(&to_insert)).await??;
        }
        MergeAction::UpdateMetadata { pinned, deleted } => {
            let existing = local.expect("UpdateMetadata implies a local clip");
            let store = Arc::clone(&ctx.store);
            let id = existing.id;
            tokio::task::spawn_blocking(move || -> clipse_store::Result<()> {
                store.set_pinned(id, pinned, hlc)?;
                if deleted {
                    store.delete(id, hlc)?;
                }
                Ok(())
            })
            .await??;
            // An un-delete cannot be applied: the store has no way to revive a
            // tombstone, so a clip deleted here stays deleted even if the peer
            // later un-deleted it. Recorded rather than silently dropped.
            if !deleted && existing.deleted {
                warn!("peer un-deleted a clip; the store cannot revive a tombstone");
            }
        }
    }

    // Recorded *after* a successful merge and before the clipboard is written,
    // so the watcher's echo of that write is suppressed rather than
    // broadcast back.
    ctx.loop_guard
        .lock()
        .expect("loop guard poisoned")
        .record_received(hash, hlc.device);

    Ok(true)
}

fn variant_name(message: &SyncMessage) -> &'static str {
    match message {
        SyncMessage::Hello { .. } => "Hello",
        SyncMessage::Summary { .. } => "Summary",
        SyncMessage::Want { .. } => "Want",
        SyncMessage::Push { .. } => "Push",
        SyncMessage::BlobOffer { .. } => "BlobOffer",
        SyncMessage::BlobWant { .. } => "BlobWant",
        SyncMessage::BlobChunk { .. } => "BlobChunk",
        SyncMessage::BlobEnd { .. } => "BlobEnd",
        SyncMessage::Ack { .. } => "Ack",
        SyncMessage::Bye { .. } => "Bye",
    }
}

/// Hashes the peer offered that we already hold, for diagnostics.
pub fn already_held(entries: &[ClipSummary], have: impl Fn(&ContentHash) -> bool) -> usize {
    entries.iter().filter(|e| have(&e.hash)).count()
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::RwLock;

    use clipse_core::{ClipFormat, ClipSource, DeviceId, Paths, Payload};
    use clipse_crypto::{DeviceIdentity, PairedDevice, Platform, Trust};
    use clipse_net::QuicTransport;
    use clipse_net::candidate::{Candidate, CandidateList};
    use clipse_store::{HistoryQuery, StoreOptions};
    use tempfile::TempDir;

    use super::*;

    /// One whole daemon's worth of state: its own store, clock, identity and
    /// transport. Two of these is a two-device deployment.
    struct TestDaemon {
        _dir: TempDir,
        ctx: SyncContext,
        identity: Arc<DeviceIdentity>,
        trust: Arc<RwLock<Trust>>,
        transport: Arc<QuicTransport>,
    }

    impl TestDaemon {
        fn new(label: &str) -> Self {
            let dir = TempDir::new().unwrap();
            let paths = Paths::with_root(dir.path());
            let store = Arc::new(Store::open(&paths, StoreOptions::default()).unwrap());

            let device = DeviceId::generate();
            let identity = Arc::new(DeviceIdentity::generate(device));
            let trust = Arc::new(RwLock::new(Trust::new(device)));
            let transport = Arc::new(
                QuicTransport::bind(
                    "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
                    Arc::clone(&identity),
                    Arc::clone(&trust),
                )
                .unwrap(),
            );

            Self {
                _dir: dir,
                ctx: SyncContext {
                    store,
                    clock: Arc::new(HlcClock::new(device)),
                    loop_guard: Arc::new(Mutex::new(LoopGuard::default())),
                    label: label.to_string(),
                    platform: "test".to_string(),
                },
                identity,
                trust,
                transport,
            }
        }

        fn id(&self) -> DeviceId {
            self.identity.device_id()
        }

        fn addr(&self) -> SocketAddr {
            self.transport.local_addr()
        }

        fn pair_with(&self, other: &TestDaemon, label: &str) {
            self.trust.write().unwrap().add_peer(PairedDevice {
                device_id: other.id(),
                static_public: other.identity.public_key(),
                label: label.to_string(),
                platform: Platform::Linux,
                addresses: vec![],
                paired_at_ms: 0,
            });
        }

        /// Store a locally-captured clip, the way `capture::run` would.
        fn capture(&self, text: &str) -> Clip {
            let clip = Clip::new(
                vec![Payload::new(ClipFormat::Text, text.as_bytes().to_vec())],
                ClipSource::new(self.ctx.clock.device(), self.ctx.label.clone()),
                self.ctx.clock.now(),
            );
            self.ctx.store.insert(&clip).unwrap();
            clip
        }

        fn history(&self) -> Vec<Clip> {
            // Not the default page size: these tests deliberately push past it.
            self.ctx
                .store
                .recent(HistoryQuery {
                    limit: 10_000,
                    ..HistoryQuery::default()
                })
                .unwrap()
        }

        fn texts(&self) -> Vec<String> {
            let mut texts: Vec<String> = self
                .history()
                .iter()
                .filter(|c| !c.deleted)
                .filter_map(|c| c.text().map(str::to_string))
                .collect();
            texts.sort();
            texts
        }
    }

    /// Run one full session between two daemons, both sides concurrently.
    async fn sync(a: &TestDaemon, b: &TestDaemon) -> (SyncOutcome, SyncOutcome) {
        let responder_transport = Arc::clone(&b.transport);
        let responder = tokio::spawn(async move {
            responder_transport
                .accept_session()
                .await
                .expect("endpoint closed")
                .expect("accept")
        });

        let mut dialler_link = a
            .transport
            .dial(b.id(), &CandidateList::new([Candidate::lan(b.addr())]))
            .await
            .expect("dial");

        let mut responder_link = responder.await.unwrap();

        // Both halves must run at the same time: the protocol alternates, so
        // one side is always waiting on the other.
        let (dialler_out, responder_out) = tokio::join!(
            run_session(&mut dialler_link, &a.ctx, Role::Dialler),
            run_session(&mut responder_link, &b.ctx, Role::Responder),
        );

        dialler_link.close("done");
        responder_link.close("done");
        (
            dialler_out.expect("dialler"),
            responder_out.expect("responder"),
        )
    }

    fn pair(a: &TestDaemon, b: &TestDaemon) {
        a.pair_with(b, "b");
        b.pair_with(a, "a");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_clip_copied_on_one_device_reaches_the_other() {
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        pair(&alice, &bob);

        alice.capture("hello from alice");

        let (from_alice, into_bob) = sync(&alice, &bob).await;
        assert_eq!(from_alice.sent, 1, "alice should have sent one clip");
        assert_eq!(into_bob.received, 1, "bob should have taken one clip");

        assert_eq!(bob.texts(), vec!["hello from alice".to_string()]);
        // And it kept its provenance, which is what the UI badge shows.
        let received = &bob.history()[0];
        assert_eq!(received.source.device_label, "alice");
        assert_eq!(received.hlc.device, alice.id());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn both_directions_move_in_one_session() {
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        pair(&alice, &bob);

        alice.capture("from alice");
        bob.capture("from bob");

        sync(&alice, &bob).await;

        let expected = vec!["from alice".to_string(), "from bob".to_string()];
        assert_eq!(alice.texts(), expected);
        assert_eq!(bob.texts(), expected, "the two histories must converge");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn syncing_twice_moves_nothing_the_second_time() {
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        pair(&alice, &bob);
        alice.capture("only once");

        let (first, _) = sync(&alice, &bob).await;
        assert_eq!(first.sent, 1);

        let (second, into_bob) = sync(&alice, &bob).await;
        assert_eq!(second.sent, 0, "a settled pair should exchange nothing");
        assert_eq!(into_bob.received, 0);
        assert_eq!(bob.texts().len(), 1, "and must not duplicate the clip");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_device_that_was_offline_catches_up_on_everything_it_missed() {
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        pair(&alice, &bob);

        for i in 0..25 {
            alice.capture(&format!("while bob was away {i}"));
        }

        let (out, _) = sync(&alice, &bob).await;
        assert_eq!(out.sent, 25);
        assert_eq!(bob.texts().len(), 25);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_summary_spanning_several_pages_still_converges() {
        // More clips than one Summary page holds, so the paging path runs.
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        pair(&alice, &bob);

        let count = SUMMARY_PAGE * 2 + 17;
        for i in 0..count {
            alice.capture(&format!("clip number {i}"));
        }

        let (out, into_bob) = sync(&alice, &bob).await;
        assert_eq!(out.sent, count);
        assert_eq!(into_bob.received, count);
        assert_eq!(bob.texts().len(), count);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_deletion_replicates() {
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        pair(&alice, &bob);

        let clip = alice.capture("delete me");
        sync(&alice, &bob).await;
        assert_eq!(bob.texts().len(), 1);

        alice
            .ctx
            .store
            .delete(clip.id, alice.ctx.clock.now())
            .unwrap();
        sync(&alice, &bob).await;

        assert!(
            bob.texts().is_empty(),
            "a deletion on one device must reach the other"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_loop_guard_stops_a_received_clip_going_straight_back() {
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        pair(&alice, &bob);

        let clip = alice.capture("round trip candidate");
        sync(&alice, &bob).await;

        // Bob's guard now knows this content came from Alice, so Bob's own
        // clipboard watcher observing the write must not push it back.
        let verdict = bob
            .ctx
            .loop_guard
            .lock()
            .unwrap()
            .verdict(&clip.hash, alice.id());
        assert_ne!(
            verdict,
            clipse_sync::RebroadcastVerdict::Send,
            "bob would have bounced the clip straight back to alice"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_unpaired_device_cannot_start_a_session_at_all() {
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        // Deliberately not paired.
        alice.capture("private");

        let error = alice
            .transport
            .dial(bob.id(), &CandidateList::new([Candidate::lan(bob.addr())]))
            .await
            .expect_err("an unpaired peer must not sync");
        assert!(!error.is_retryable());
        assert!(bob.texts().is_empty());
    }
}
