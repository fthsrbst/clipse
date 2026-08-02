//! Driving one sync session to completion.
//!
//! Strictly alternating turns, dialler first — see `docs/sync-protocol.md` §5
//! for why a symmetric exchange deadlocks once both summaries outgrow the QUIC
//! flow-control window.
//!
//! # Blobs
//!
//! Payloads too big to inline follow their clips in the same turn, on their own
//! unidirectional streams. The **receiver** drives that exchange: having just
//! applied the clips, it already knows exactly which digests it is missing, so
//! it sends one list and then reads that many transfers in the same order. The
//! sender needs no separate offer round and neither side needs a terminator
//! message — the length of the list is the terminator.

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
    /// Where a clip that arrived from a peer is announced to the UIs. A clip
    /// merged straight into the store is invisible until something re-queries,
    /// which for a user watching the history window is indistinguishable from
    /// sync not working at all. `None` in the tests here, which have no UI.
    pub events: Option<tokio::sync::broadcast::Sender<clipse_ipc::Event>>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub sent: usize,
    pub received: usize,
    pub rejected: usize,
    pub blobs_sent: usize,
    pub blobs_received: usize,
    pub blobs_rejected: usize,
    /// The newest clip this session took in, if any. The caller decides what
    /// to do with it; the protocol has no opinion. Only *new content* counts —
    /// a pin or a tombstone arriving is not something to put on a clipboard.
    pub newest_received: Option<clipse_core::ClipId>,
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

    // Read in one hop rather than one per hash. A `spawn_blocking` per clip
    // means a thread handoff per clip, which on a first sync of a few hundred
    // clips is most of the session's wall clock and none of its work.
    //
    // Served from the store rather than from the page just built: the history
    // may have changed since, and the store is the truth.
    let store = Arc::clone(&ctx.store);
    let found = tokio::task::spawn_blocking(move || {
        wanted
            .iter()
            .map(|hash| store.by_hash_including_deleted(hash))
            .collect::<clipse_store::Result<Vec<_>>>()
    })
    .await??;

    for clip in found {
        match clip {
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

    // The receiver has applied those clips and now says which blob payloads it
    // still lacks. An empty list is sent too, so this step is unconditional on
    // both sides.
    let blob_digests = match link.recv().await? {
        SyncMessage::Want { hashes } => hashes,
        other => {
            return Err(SyncError::Unexpected {
                expected: "Want (blobs)",
                got: variant_name(&other).to_string(),
            });
        }
    };

    for digest in &blob_digests {
        // One at a time here, unlike the summary walk above: a blob is measured
        // in megabytes, and reading them all into memory before sending any
        // would trade a thread handoff for the whole transfer's footprint.
        let store = Arc::clone(&ctx.store);
        let digest = *digest;
        let bytes = tokio::task::spawn_blocking(move || store.read_blob(&digest)).await?;
        match bytes {
            Ok(bytes) => {
                link.send_blob(&bytes).await?;
                outcome.blobs_sent += 1;
            }
            Err(e) => {
                // The receiver is waiting for exactly this many streams, so a
                // blob we cannot read still has to produce one — an empty one,
                // which fails the receiver's digest check and is discarded.
                warn!(error = %e, "a requested blob could not be read; sending nothing");
                link.send_blob(&[]).await?;
            }
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

    // The whole summary is compared in one hop off the runtime — see the note
    // in `offer_our_history` about what a `spawn_blocking` per clip costs.
    let store = Arc::clone(&ctx.store);
    let summary: Vec<(ContentHash, Hlc)> = entries
        .iter()
        .map(|entry| (entry.hash, entry.hlc))
        .collect();
    let wanted = tokio::task::spawn_blocking(move || -> clipse_store::Result<Vec<ContentHash>> {
        let mut wanted = Vec::new();
        for (hash, hlc) in summary {
            let take = match store.by_hash_including_deleted(&hash)? {
                None => true,
                Some(existing) => hlc > existing.hlc,
            };
            if take {
                wanted.push(hash);
            }
        }
        Ok(wanted)
    })
    .await??;

    let expected = wanted.len();
    link.send(&SyncMessage::Want { hashes: wanted }).await?;

    let mut missing_blobs: Vec<ContentHash> = Vec::new();
    // Which of the inserts was newest by HLC, not by arrival order — a summary
    // is walked in whatever order the store returned it.
    let mut newest: Option<(Hlc, clipse_core::ClipId)> = None;
    for _ in 0..expected {
        match link.recv().await? {
            SyncMessage::Push { clip } => {
                let blob_digests: Vec<ContentHash> = clip
                    .payloads
                    .iter()
                    .filter(|p| p.is_blob())
                    .map(|p| p.digest)
                    .collect();
                let hlc = clip.hlc;
                match apply(ctx, peer, *clip).await? {
                    Applied::Inserted(id) => {
                        outcome.received += 1;
                        missing_blobs.extend(blob_digests);
                        if newest.is_none_or(|(seen, _)| hlc > seen) {
                            newest = Some((hlc, id));
                        }
                    }
                    Applied::Updated => {
                        outcome.received += 1;
                        missing_blobs.extend(blob_digests);
                    }
                    Applied::Refused => outcome.rejected += 1,
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

    // Ask for the payload bytes of everything just applied that we do not
    // already hold. Order matters: the sender streams them back in this order.
    let store = Arc::clone(&ctx.store);
    let needed: Vec<ContentHash> =
        tokio::task::spawn_blocking(move || -> clipse_store::Result<Vec<ContentHash>> {
            let mut needed: Vec<ContentHash> = Vec::new();
            for digest in missing_blobs {
                if !store.has_blob(&digest)? && !needed.contains(&digest) {
                    needed.push(digest);
                }
            }
            Ok(needed)
        })
        .await??;

    link.send(&SyncMessage::Want {
        hashes: needed.clone(),
    })
    .await?;

    for digest in &needed {
        let bytes = link.recv_blob().await?;
        if ContentHash::of(&bytes) != *digest {
            // Discarded whole rather than stored partially: the clip stays
            // incomplete and the next session asks again.
            warn!("a blob did not match its digest; discarding");
            outcome.blobs_rejected += 1;
            continue;
        }
        let store = Arc::clone(&ctx.store);
        let digest = *digest;
        tokio::task::spawn_blocking(move || store.put_blob(&digest, &bytes)).await??;
        outcome.blobs_received += 1;
    }

    // Reported only now, after the blob transfers: a clip whose payload is
    // still missing is not something to hand to a clipboard.
    outcome.newest_received = newest.map(|(_, id)| id);

    let store = Arc::clone(&ctx.store);
    let max_hlc = tokio::task::spawn_blocking(move || store.max_hlc()).await??;
    link.send(&SyncMessage::Ack {
        hlc: max_hlc.unwrap_or_else(|| ctx.clock.now()),
    })
    .await?;

    Ok(())
}

/// What merging one incoming clip did.
enum Applied {
    /// Content this device had never seen.
    Inserted(clipse_core::ClipId),
    /// A pin or a tombstone landing on a clip already held.
    Updated,
    Refused,
}

/// Merge one incoming clip, and tell the UIs about it.
async fn apply(ctx: &SyncContext, peer: DeviceId, clip: Clip) -> Result<Applied, SyncError> {
    let store = Arc::clone(&ctx.store);
    let hash = clip.hash;
    let local =
        tokio::task::spawn_blocking(move || store.by_hash_including_deleted(&hash)).await??;

    let action = merge(local.as_ref(), &clip);
    let hlc = clip.hlc;

    let applied = match action {
        MergeAction::Reject => {
            warn!(peer = %peer.short(), "refused a clip that does not hash to its payloads");
            return Ok(Applied::Refused);
        }
        MergeAction::Ignore => return Ok(Applied::Refused),
        MergeAction::Insert => {
            let store = Arc::clone(&ctx.store);
            let to_insert = clip.clone();
            tokio::task::spawn_blocking(move || store.insert(&to_insert)).await??;
            let id = clip.id;
            emit(ctx, clipse_ipc::Event::ClipAdded(Box::new(clip)));
            Applied::Inserted(id)
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

            if deleted {
                emit(ctx, clipse_ipc::Event::ClipRemoved(id));
            } else {
                // Re-read rather than patching the local copy: the store owns
                // what a row now says, and a stale one would be published.
                let store = Arc::clone(&ctx.store);
                if let Ok(Some(updated)) =
                    tokio::task::spawn_blocking(move || store.get(id)).await?
                {
                    emit(ctx, clipse_ipc::Event::ClipUpdated(Box::new(updated)));
                }
            }
            Applied::Updated
        }
    };

    // Recorded *after* a successful merge and before the clipboard is written,
    // so the watcher's echo of that write is suppressed rather than
    // broadcast back.
    ctx.loop_guard
        .lock()
        .expect("loop guard poisoned")
        .record_received(hash, hlc.device);

    Ok(applied)
}

/// Publish to whoever is subscribed. A daemon with no UI attached is the
/// normal case, not an error.
fn emit(ctx: &SyncContext, event: clipse_ipc::Event) {
    if let Some(events) = &ctx.events {
        let _ = events.send(event);
    }
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
        events: tokio::sync::broadcast::Sender<clipse_ipc::Event>,
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

            let (events, _) = tokio::sync::broadcast::channel(256);

            Self {
                _dir: dir,
                ctx: SyncContext {
                    store,
                    clock: Arc::new(HlcClock::new(device)),
                    loop_guard: Arc::new(Mutex::new(LoopGuard::default())),
                    label: label.to_string(),
                    platform: "test".to_string(),
                    events: Some(events.clone()),
                },
                identity,
                trust,
                transport,
                events,
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

        /// Store a clip whose payload spilled to the blob store, the way
        /// `capture::run` does for a screenshot.
        fn capture_blob(&self, seed: u8, len: usize) -> (Clip, Vec<u8>) {
            let bytes: Vec<u8> = (0..len).map(|i| (i as u8) ^ seed).collect();
            let payload = Payload::new(ClipFormat::Png, bytes.clone());
            assert!(payload.is_blob(), "test premise: this must spill to a blob");
            let digest = payload.digest;

            let clip = Clip::new(
                vec![payload],
                ClipSource::new(self.ctx.clock.device(), self.ctx.label.clone()),
                self.ctx.clock.now(),
            );
            self.ctx.store.put_blob(&digest, &bytes).unwrap();
            self.ctx.store.insert(&clip).unwrap();
            (clip, bytes)
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

        dialler_link.close_gracefully("done").await;
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

    /// The whole product, from a user's seat: a clip that arrives from another
    /// device has to be *announced*, not just stored. Merging it straight into
    /// the store leaves the history window showing yesterday's list, which
    /// reads as "sync is broken" however well the protocol ran.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_clip_that_arrives_from_a_peer_is_announced_to_the_uis() {
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        pair(&alice, &bob);

        // Subscribed before the session, the way a running UI would be.
        let mut watching = bob.events.subscribe();
        alice.capture("something worth showing");

        let (_, into_bob) = sync(&alice, &bob).await;
        assert_eq!(into_bob.received, 1);

        match watching.try_recv() {
            Ok(clipse_ipc::Event::ClipAdded(clip)) => {
                assert_eq!(clip.text(), Some("something worth showing"));
            }
            other => panic!("bob's UI was told nothing about the clip: {other:?}"),
        }
    }

    /// And it is the newest one that gets offered to the clipboard — a device
    /// catching up after a day away must not replay a whole history through
    /// it, and must not land on whichever clip the summary happened to end on.
    #[tokio::test(flavor = "multi_thread")]
    async fn only_the_newest_arrival_is_offered_to_the_clipboard() {
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        pair(&alice, &bob);

        alice.capture("first");
        alice.capture("second");
        let last = alice.capture("newest of the three");

        let (_, into_bob) = sync(&alice, &bob).await;
        assert_eq!(into_bob.received, 3);
        assert_eq!(
            into_bob.newest_received,
            Some(last.id),
            "the clipboard would have been handed the wrong clip"
        );
    }

    /// A pin or a tombstone is metadata, not content: there is nothing to put
    /// on a clipboard, and doing so would resurrect a deleted clip there.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_deletion_arriving_is_not_offered_to_the_clipboard() {
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        pair(&alice, &bob);

        let clip = alice.capture("delete me");
        sync(&alice, &bob).await;

        alice
            .ctx
            .store
            .delete(clip.id, alice.ctx.clock.now())
            .unwrap();
        let (_, into_bob) = sync(&alice, &bob).await;

        assert_eq!(into_bob.received, 1, "the tombstone should have replicated");
        assert_eq!(into_bob.newest_received, None);
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

    #[tokio::test(flavor = "multi_thread")]
    async fn a_large_payload_arrives_with_its_bytes_not_just_its_row() {
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        pair(&alice, &bob);

        // Comfortably over INLINE_MAX_BYTES, so it travels as a blob on its
        // own stream rather than inside the Push.
        let (clip, bytes) = alice.capture_blob(0xA5, 300_000);

        let (out, into_bob) = sync(&alice, &bob).await;
        assert_eq!(out.sent, 1);
        assert_eq!(out.blobs_sent, 1, "the payload must actually be sent");
        assert_eq!(into_bob.blobs_received, 1);
        assert_eq!(into_bob.blobs_rejected, 0);

        // The row arrived...
        let received = bob
            .ctx
            .store
            .by_hash(&clip.hash)
            .unwrap()
            .expect("clip row");
        // ...and so did the bytes, unchanged.
        let digest = received.payloads[0].digest;
        assert!(
            bob.ctx.store.has_blob(&digest).unwrap(),
            "blob bytes missing"
        );
        assert_eq!(bob.ctx.store.read_blob(&digest).unwrap(), bytes);
        assert!(
            received.is_complete(|d| bob.ctx.store.has_blob(d).unwrap()),
            "the clip should no longer report itself incomplete"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_blob_already_held_is_not_sent_again() {
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        pair(&alice, &bob);
        alice.capture_blob(0x11, 200_000);

        let (first, _) = sync(&alice, &bob).await;
        assert_eq!(first.blobs_sent, 1);

        let (second, into_bob) = sync(&alice, &bob).await;
        assert_eq!(second.blobs_sent, 0, "re-sending 200 KB every session");
        assert_eq!(into_bob.blobs_received, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_clip_mixing_inline_and_blob_payloads_survives_intact() {
        let alice = TestDaemon::new("alice");
        let bob = TestDaemon::new("bob");
        pair(&alice, &bob);

        // A screenshot pasted with a caption: one payload rides inside the
        // Push, the other on its own stream.
        let big: Vec<u8> = (0..250_000).map(|i| (i % 253) as u8).collect();
        let clip = Clip::new(
            vec![
                Payload::new(ClipFormat::Text, b"a caption".to_vec()),
                Payload::new(ClipFormat::Png, big.clone()),
            ],
            ClipSource::new(alice.ctx.clock.device(), "alice".to_string()),
            alice.ctx.clock.now(),
        );
        let png_digest = clip
            .payloads
            .iter()
            .find(|p| p.format == ClipFormat::Png)
            .unwrap()
            .digest;
        alice.ctx.store.put_blob(&png_digest, &big).unwrap();
        alice.ctx.store.insert(&clip).unwrap();

        sync(&alice, &bob).await;

        let received = bob.ctx.store.by_hash(&clip.hash).unwrap().expect("clip");
        assert_eq!(received.text(), Some("a caption"), "inline half lost");
        assert_eq!(
            bob.ctx.store.read_blob(&png_digest).unwrap(),
            big,
            "blob half lost"
        );
    }
}
