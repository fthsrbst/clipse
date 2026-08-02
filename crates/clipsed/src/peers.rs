//! Keeping sessions with the paired devices alive.
//!
//! Two loops. One accepts whatever arrives on the QUIC endpoint; the other
//! syncs when there is a reason to — a local copy, a peer just seen — and
//! falls back to a slow timer when there is not. Both end up in the same
//! place — `sync::run_session` — differing only in which side of the
//! alternation they take.
//!
//! # Why the dial loop is not a poll loop
//!
//! It used to be one: a 30-second tick, and nothing else. That made the
//! product's central promise — copy here, paste there — take up to half a
//! minute, which reads as "sync is broken" no matter how fast the session
//! itself runs. The tick is still here as a floor for peers that appear
//! without announcing themselves, but the normal path is [`PeerManager::nudge`]
//! from the capture path: a clip lands in the store and the sync starts in the
//! same millisecond.
//!
//! Failures are not all equal. A peer that cannot be *reached* is ordinary and
//! gets exponential backoff; a peer that *refuses* us has either been removed
//! or is something worth a human looking at, so it is surfaced and not retried
//! until the paired set changes. That distinction lives in
//! `clipse_net::DialError::is_retryable`, and honouring it is the whole reason
//! this module keeps per-peer state.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use clipse_core::DeviceId;
use clipse_crypto::{CandidateAddress, Trust};
use clipse_ipc::protocol::{Connectivity, PeerInfo};
use clipse_net::candidate::{Candidate, CandidateList};
use clipse_net::{
    Backoff, Discovery, DiscoveryEvent, Inbound, PairingCall, QuicTransport, Reachability,
};
use tokio::sync::{Notify, watch};
use tracing::{debug, info, warn};

use crate::sync::{self, Role, SyncContext};

/// How often the dial loop wakes up when nothing has happened. A floor on how
/// stale a peer can get — the normal path is [`PeerManager::nudge`], which
/// fires the moment something is copied.
const DIAL_TICK: Duration = Duration::from_secs(30);

/// How long after the last successful session a peer still counts as online.
/// Comfortably more than [`DIAL_TICK`] so an idle-but-reachable peer does not
/// flicker offline between two ticks.
const ONLINE_WINDOW_MS: u64 = 90_000;

/// How long the whole inbound pairing ceremony may take. Three round trips on
/// a LAN is milliseconds; this only exists so a connection that opens, sends a
/// lookup and then goes quiet cannot occupy the pairing state forever.
const PAIRING_CEREMONY_TIMEOUT: Duration = Duration::from_secs(20);

/// Per-peer state the loops share.
struct PeerState {
    candidates: CandidateList,
    backoff: Backoff,
    /// Set when the peer refused us. Cleared when the paired set changes.
    refused: Option<String>,
    /// When a session with this peer last completed, and over what. Drives the
    /// devices list; `None` means "not since this daemon started".
    last_seen_ms: Option<u64>,
    last_reachability: Option<Reachability>,
}

impl PeerState {
    fn new(candidates: CandidateList) -> Self {
        Self {
            candidates,
            backoff: Backoff::default(),
            refused: None,
            last_seen_ms: None,
            last_reachability: None,
        }
    }
}

pub struct PeerManager {
    transport: Arc<QuicTransport>,
    ctx: Arc<SyncContext>,
    trust: Arc<RwLock<Trust>>,
    peers: Mutex<HashMap<DeviceId, PeerState>>,
    /// Absent when mDNS could not start — a container without multicast, a
    /// locked-down network. Sync still works through the addresses recorded at
    /// pairing time, so this is a degradation and not a failure.
    discovery: Option<Arc<Mutex<Discovery>>>,
    /// Shared with the daemon rather than owned here, so an inbound pairing
    /// attempt and the IPC request that authorised it are looking at one state
    /// machine and not two.
    pairing: Arc<tokio::sync::Mutex<crate::pairing::PairingState>>,
    events: Option<tokio::sync::broadcast::Sender<clipse_ipc::Event>>,
    /// Weak on purpose: the daemon owns this manager, so an `Arc` back would
    /// be a cycle that never drops. Used for the two things a session cannot
    /// decide for itself — whether an arriving clip belongs on this machine's
    /// clipboard, and whether a completed ceremony may add a device to the
    /// trust set on disk.
    daemon: std::sync::Weak<crate::daemon::Daemon>,
    /// Raised whenever there is a new reason to sync. One permit is kept when
    /// nobody is waiting, so a copy that lands mid-session still causes one
    /// more pass rather than being lost.
    nudge: Notify,
    /// Passes over the peer set since startup. Only read by the tests, which
    /// is the only way to state "a copy caused a sync now, not in 30 seconds"
    /// as something a machine checks.
    passes: std::sync::atomic::AtomicU64,
    /// Held for the length of one pass over the peers, so a burst of copies
    /// queues one follow-up pass instead of starting several overlapping ones
    /// against the same device.
    pass: tokio::sync::Mutex<()>,
}

impl PeerManager {
    pub fn new(
        transport: Arc<QuicTransport>,
        ctx: Arc<SyncContext>,
        trust: Arc<RwLock<Trust>>,
    ) -> Arc<Self> {
        let manager = Arc::new(Self {
            transport,
            ctx,
            trust,
            peers: Mutex::new(HashMap::new()),
            discovery: None,
            pairing: Arc::new(tokio::sync::Mutex::new(
                crate::pairing::PairingState::default(),
            )),
            events: None,
            daemon: std::sync::Weak::new(),
            nudge: Notify::new(),
            passes: std::sync::atomic::AtomicU64::new(0),
            pass: tokio::sync::Mutex::new(()),
        });
        manager.reload_from_trust();
        manager
    }

    /// Something worth sending happened. Wakes the dial loop now instead of at
    /// the next tick — this is what makes a copy on one machine show up on the
    /// other in milliseconds rather than in up to half a minute.
    pub fn nudge(&self) {
        self.nudge.notify_one();
    }

    /// Share the pairing state machine and the event channel with the daemon,
    /// so an inbound ceremony can be authorised and reported.
    pub fn with_pairing(
        mut self: Arc<Self>,
        pairing: Arc<tokio::sync::Mutex<crate::pairing::PairingState>>,
        events: tokio::sync::broadcast::Sender<clipse_ipc::Event>,
    ) -> Arc<Self> {
        if let Some(manager) = Arc::get_mut(&mut self) {
            manager.pairing = pairing;
            manager.events = Some(events);
        }
        self
    }

    /// Let finished sessions reach the daemon, which is what decides whether
    /// an arriving clip goes on the clipboard.
    pub fn with_daemon(
        mut self: Arc<Self>,
        daemon: std::sync::Weak<crate::daemon::Daemon>,
    ) -> Arc<Self> {
        if let Some(manager) = Arc::get_mut(&mut self) {
            manager.daemon = daemon;
        }
        self
    }

    /// The one thing that happens after a session either side of the dial.
    async fn settle(&self, outcome: &sync::SyncOutcome) {
        if let Some(id) = outcome.newest_received
            && let Some(daemon) = self.daemon.upgrade()
        {
            daemon.apply_incoming(id).await;
        }
    }

    /// Start announcing this device and browsing for the others.
    pub fn with_discovery(mut self: Arc<Self>, discovery: Discovery) -> Arc<Self> {
        if let Some(manager) = Arc::get_mut(&mut self) {
            manager.discovery = Some(Arc::new(Mutex::new(discovery)));
        }
        self
    }

    /// One browse, folding whatever is found into the peers we already trust.
    ///
    /// Discovery never *adds* a peer: an address is only useful for a device
    /// the user already paired with, and treating an advertisement as anything
    /// more would let anyone on the LAN join by shouting.
    async fn refresh_from_discovery(&self) {
        let Some(discovery) = self.discovery.clone() else {
            return;
        };

        // `sweep` blocks on a channel, so it does not belong on the runtime.
        let found = tokio::task::spawn_blocking(move || {
            discovery
                .lock()
                .expect(POISONED)
                .sweep(Duration::from_millis(1_500))
        })
        .await;

        let events = match found {
            Ok(Ok(events)) => events,
            Ok(Err(e)) => {
                debug!(error = %e, "discovery sweep failed");
                return;
            }
            Err(e) => {
                debug!(error = %e, "discovery task failed");
                return;
            }
        };

        let now = now_ms();
        let mut peers = self.peers.lock().expect(POISONED);
        for event in events {
            match event {
                DiscoveryEvent::Found(peer) => {
                    if let Some(state) = peers.get_mut(&peer.device) {
                        state.candidates.refresh_lan(peer.addresses, now);
                        // Seeing a device on the network is a reason to try it
                        // again, whatever happened last time.
                        state.backoff.reset();
                    } else {
                        debug!(device = %peer.device.short(), "ignoring an unpaired device");
                    }
                }
                DiscoveryEvent::Lost(device) => {
                    // Nothing to do to the candidates. A peer that went away
                    // is unreachable at that address whether or not we keep
                    // it, the dial fails fast and stays retryable, and the
                    // entry sinks in `dial_order` on its own as others are
                    // seen. Deleting it is how a peer that mDNS stops
                    // re-reporting becomes permanently unreachable.
                    debug!(device = %device.short(), "peer left the network");
                }
                DiscoveryEvent::Incompatible { instance, reason } => {
                    warn!(
                        instance,
                        reason, "a Clipse device on this network cannot be talked to"
                    );
                }
            }
        }
    }

    /// Rebuild the peer table from the paired set — on startup, and whenever
    /// pairing adds or removes a device.
    pub fn reload_from_trust(&self) {
        let known: Vec<(DeviceId, CandidateList)> = {
            let trust = self.trust.read().expect(POISONED);
            trust
                .peers()
                .map(|peer| (peer.device_id, candidates_of(&peer.addresses)))
                .collect()
        };

        let mut peers = self.peers.lock().expect(POISONED);
        peers.retain(|id, _| known.iter().any(|(known_id, _)| known_id == id));
        for (id, candidates) in known {
            peers
                .entry(id)
                .and_modify(|state| {
                    // A device that was re-paired deserves a fresh chance.
                    state.refused = None;
                    for candidate in candidates.iter() {
                        state.candidates.upsert(candidate.clone());
                    }
                })
                .or_insert_with(|| PeerState::new(candidates));
        }
    }

    /// Accept inbound connections until the endpoint closes.
    pub async fn accept_loop(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        loop {
            let inbound = tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() { return; }
                    continue;
                }
                inbound = self.transport.accept() => inbound,
            };

            let Some(inbound) = inbound else {
                debug!("quic endpoint closed; accept loop ending");
                return;
            };

            match inbound {
                Ok(Inbound::Session(link)) => {
                    let this = Arc::clone(&self);
                    tokio::spawn(async move {
                        let mut link = *link;
                        let peer = link.remote_device();
                        match sync::run_session(&mut link, &this.ctx, Role::Responder).await {
                            Ok(outcome) => {
                                info!(peer = %peer.short(), ?outcome, "served a sync session");
                                let info = link.info();
                                this.note_success(peer, info.addr, info.reachability);
                                this.settle(&outcome).await;
                            }
                            Err(e) => {
                                warn!(peer = %peer.short(), error = %e, "sync session failed")
                            }
                        }
                        link.close("done");
                    });
                }
                Ok(Inbound::Pairing(exchange)) => {
                    let this = Arc::clone(&self);
                    tokio::spawn(async move { this.serve_pairing(exchange).await });
                }
                Err(e) => debug!(error = %e, "inbound connection ended"),
            }
        }
    }

    /// Sync when there is a reason to, forever.
    ///
    /// The first pass runs immediately rather than after a tick: a daemon that
    /// has just started is exactly the case where the history is most likely to
    /// be behind, and waiting 30 seconds to find that out is the old bug in
    /// miniature.
    pub async fn dial_loop(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        self.refresh_from_discovery().await;
        self.sync_all().await;

        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() { return; }
                }
                // A local capture, a pairing, a peer coming back: sync now, and
                // do not spend 1.5 seconds browsing mDNS first.
                _ = self.nudge.notified() => self.sync_all().await,
                _ = tokio::time::sleep(DIAL_TICK) => {
                    self.refresh_from_discovery().await;
                    self.sync_all().await;
                }
            }
        }
    }

    /// One pass over every paired device.
    ///
    /// Serialised against other passes: two overlapping passes would dial the
    /// same peer twice, and the second would find nothing to send anyway.
    pub async fn sync_all(&self) {
        let _pass = self.pass.lock().await;
        self.passes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let due: Vec<DeviceId> = {
            let peers = self.peers.lock().expect(POISONED);
            peers
                .iter()
                .filter(|(_, state)| state.refused.is_none())
                .map(|(id, _)| *id)
                .collect()
        };

        for peer in due {
            if let Err(e) = self.sync_one(peer).await {
                debug!(peer = %peer.short(), error = %e, "sync attempt did not complete");
            }
        }
    }

    /// Dial one peer and run a session. Records backoff or refusal.
    pub async fn sync_one(&self, peer: DeviceId) -> Result<(), String> {
        let candidates = {
            let peers = self.peers.lock().expect(POISONED);
            match peers.get(&peer) {
                Some(state) if state.refused.is_none() => state.candidates.clone(),
                Some(state) => return Err(state.refused.clone().unwrap_or_default()),
                None => return Err("not paired".to_string()),
            }
        };

        match self.transport.dial(peer, &candidates).await {
            Ok(mut link) => {
                let (addr, reachability) = {
                    let info = link.info();
                    (info.addr, info.reachability)
                };
                let result = sync::run_session(&mut link, &self.ctx, Role::Dialler).await;
                // Gracefully, because the dialler's last act is a send: see
                // `PeerLink::close_gracefully`.
                link.close_gracefully("done").await;

                match result {
                    Ok(outcome) => {
                        info!(peer = %peer.short(), ?outcome, "synced");
                        self.note_success(peer, addr, reachability);
                        self.settle(&outcome).await;
                        Ok(())
                    }
                    Err(e) => Err(e.to_string()),
                }
            }
            Err(error) => {
                let message = error.to_string();
                // Which addresses were tried, and what each said. Without this
                // an unreachable peer is a one-line summary, and the answer to
                // "why" — a stale port, a link-local address that cannot be
                // dialled — is not in the log at all.
                let tried = match &error {
                    clipse_net::DialError::Unreachable { attempts } => attempts
                        .iter()
                        .map(|a| format!("{} ({})", a.addr, a.reason))
                        .collect::<Vec<_>>()
                        .join(", "),
                    _ => String::new(),
                };
                let mut peers = self.peers.lock().expect(POISONED);
                if let Some(state) = peers.get_mut(&peer) {
                    if error.is_retryable() {
                        let delay = state.backoff.next_delay_ms();
                        debug!(peer = %peer.short(), delay_ms = delay, %tried, "peer unreachable");
                    } else {
                        // Surfaced rather than retried: this is a removed
                        // device or something that needs attention.
                        warn!(peer = %peer.short(), error = %message, "peer refused us");
                        state.refused = Some(message.clone());
                    }
                }
                Err(message)
            }
        }
    }

    fn emit(&self, event: clipse_ipc::Event) {
        if let Some(events) = &self.events {
            let _ = events.send(event);
        }
    }

    /// Serve the offering half of one pairing ceremony.
    ///
    /// Every failure ends the same way — the connection is dropped without a
    /// reason — because the difference between "no window is open", "that is
    /// not the code on my screen" and "your proof did not verify" is exactly
    /// what someone guessing would like to know.
    async fn serve_pairing(self: Arc<Self>, mut exchange: clipse_net::PairingExchange) {
        let addr = exchange.remote_addr();
        match tokio::time::timeout(
            PAIRING_CEREMONY_TIMEOUT,
            self.pairing_ceremony(&mut exchange),
        )
        .await
        {
            Ok(Ok(())) => exchange.finish().await,
            Ok(Err(reason)) => {
                debug!(%addr, %reason, "an inbound pairing attempt ended");
                exchange.reject();
            }
            Err(_) => {
                debug!(%addr, "an inbound pairing attempt stalled");
                self.pairing.lock().await.cancel();
                exchange.reject();
            }
        }
    }

    async fn pairing_ceremony(
        &self,
        exchange: &mut clipse_net::PairingExchange,
    ) -> Result<(), String> {
        use crate::pairing::LookupOutcome;
        use clipse_crypto::PairingWire;

        // 1. Is this code ours?
        let PairingWire::Lookup { tag } =
            PairingWire::from_bytes(exchange.first()).map_err(|e| e.to_string())?
        else {
            return Err("a pairing connection opened with the wrong message".into());
        };

        let offer = match self.pairing.lock().await.lookup(&tag) {
            LookupOutcome::Offer(offer) => offer,
            LookupOutcome::Refuse => {
                // Answered rather than dropped: the device doing the typing is
                // walking every Clipse on the network, and a clean "not me"
                // lets it move on to the next one immediately.
                let _ = exchange.reply(&PairingWire::Refused.to_bytes()).await;
                return Ok(());
            }
        };
        exchange
            .reply(&PairingWire::Offer(offer).to_bytes())
            .await
            .map_err(|e| e.to_string())?;

        // 2. Their identity and nonce; our proof back.
        let PairingWire::Accept(accept) =
            PairingWire::from_bytes(&exchange.next().await.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?
        else {
            return Err("expected a pairing accept".into());
        };
        let confirm = self
            .pairing
            .lock()
            .await
            .answer_accept(&accept)
            .map_err(|e| e.to_string())?;
        exchange
            .reply(&PairingWire::Confirm(Box::new(confirm)).to_bytes())
            .await
            .map_err(|e| e.to_string())?;

        // 3. Their proof. Until this verifies, nobody has been trusted.
        let PairingWire::Finish(finish) =
            PairingWire::from_bytes(&exchange.next().await.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?
        else {
            return Err("expected a pairing proof".into());
        };
        let peer = self
            .pairing
            .lock()
            .await
            .finish(&finish)
            .map_err(|e| e.to_string())?;

        let label = peer.label.clone();
        let daemon = self
            .daemon
            .upgrade()
            .ok_or_else(|| "the daemon is shutting down".to_string())?;
        daemon.commit_pairing(peer)?;

        exchange
            .reply(&PairingWire::Done.to_bytes())
            .await
            .map_err(|e| e.to_string())?;
        info!(peer = %label, "paired");
        self.emit(clipse_ipc::Event::PairingSucceeded { peer_label: label });
        // A brand new peer has a whole history to exchange; there is no reason
        // to make the user wait for the next tick to see it.
        self.nudge();
        Ok(())
    }

    /// Every address worth asking "are you showing this code?".
    ///
    /// mDNS finds devices on the LAN; the tailnet is walked separately because
    /// it carries no multicast, so a tailnet peer is only reachable at
    /// [`clipse_net::DEFAULT_SYNC_PORT`] — which is exactly why the endpoint
    /// tries to bind that port rather than an ephemeral one.
    pub async fn pairing_targets(&self) -> Vec<SocketAddr> {
        let mut targets: Vec<SocketAddr> = Vec::new();

        if let Some(discovery) = self.discovery.clone() {
            let found = tokio::task::spawn_blocking(move || {
                discovery
                    .lock()
                    .expect(POISONED)
                    .sweep(Duration::from_millis(1_200))
            })
            .await;
            if let Ok(Ok(events)) = found {
                for event in events {
                    if let DiscoveryEvent::Found(peer) = event {
                        targets.extend(peer.addresses);
                    }
                }
            }
        }

        let tailnet = tokio::task::spawn_blocking(clipse_net::TailnetStatus::query).await;
        if let Ok(Ok(status)) = tailnet
            && status.is_running()
        {
            for peer in status.peers.iter().filter(|peer| peer.online) {
                if let Some(ip) = peer.preferred_ip() {
                    targets.push(SocketAddr::new(ip, clipse_net::DEFAULT_SYNC_PORT));
                }
            }
        }

        // A device that is already paired can be re-paired (a reinstall on the
        // other side), and its recorded address is worth trying even when
        // nothing announced it.
        {
            let peers = self.peers.lock().expect(POISONED);
            for state in peers.values() {
                targets.extend(state.candidates.iter().map(|candidate| candidate.addr));
            }
        }

        targets.sort();
        targets.dedup();
        targets
    }

    /// Open the typing half of a pairing ceremony against one address.
    pub async fn pairing_call(&self, addr: SocketAddr) -> Result<PairingCall, String> {
        self.transport
            .pairing_call(addr)
            .await
            .map_err(|e| e.to_string())
    }

    fn note_success(&self, peer: DeviceId, addr: SocketAddr, reachability: Reachability) {
        {
            let mut peers = self.peers.lock().expect(POISONED);
            if let Some(state) = peers.get_mut(&peer) {
                state.backoff.reset();
                state.refused = None;
                state.last_seen_ms = Some(now_ms());
                state.last_reachability = Some(reachability);
                state
                    .candidates
                    .upsert(Candidate::lan(addr).seen_at(now_ms()));
            }
        }
        // The tray and the devices list show "last seen"; without this they
        // would keep showing whatever was true when the window opened.
        if let Some(info) = self.peer_info(&peer) {
            self.emit(clipse_ipc::Event::DeviceChanged(info));
        }
    }

    /// How many peers are paired, and how many are not currently refusing us.
    pub fn counts(&self) -> (u32, u32) {
        let peers = self.peers.lock().expect(POISONED);
        let total = peers.len() as u32;
        let healthy = peers.values().filter(|s| s.refused.is_none()).count() as u32;
        (healthy, total)
    }

    /// The paired devices, as the UI shows them.
    ///
    /// Built from the trust set (which owns the label, the platform and the
    /// fact of pairing) joined with the live peer table (which owns whether
    /// anyone has answered lately). Answering this from the trust set alone
    /// would list devices with no idea whether they are reachable, which is the
    /// only thing a user actually wants from this list.
    pub fn peer_infos(&self) -> Vec<PeerInfo> {
        let paired: Vec<clipse_crypto::PairedDevice> = {
            let trust = self.trust.read().expect(POISONED);
            trust.peers().cloned().collect()
        };
        let peers = self.peers.lock().expect(POISONED);
        paired
            .into_iter()
            .map(|peer| info_for(&peer, peers.get(&peer.device_id)))
            .collect()
    }

    fn peer_info(&self, device: &DeviceId) -> Option<PeerInfo> {
        let paired = {
            let trust = self.trust.read().expect(POISONED);
            trust.peers().find(|p| &p.device_id == device).cloned()?
        };
        let peers = self.peers.lock().expect(POISONED);
        Some(info_for(&paired, peers.get(device)))
    }

    /// How many passes over the peer set have run. Test-only: see `passes`.
    #[cfg(test)]
    fn passes(&self) -> u64 {
        self.passes.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Why a peer is not being dialled, if it is not. Read by the tests and by
    /// the devices list once pairing is exposed over IPC.
    #[allow(dead_code, reason = "surfaced by the devices list when pairing lands")]
    pub fn refusal(&self, peer: &DeviceId) -> Option<String> {
        self.peers
            .lock()
            .expect(POISONED)
            .get(peer)
            .and_then(|s| s.refused.clone())
    }
}

const POISONED: &str = "peer table poisoned by an earlier panic";

/// One paired device as the UI sees it.
///
/// "Online" is deliberately "a session completed recently" rather than "an
/// address is known": an address that has not answered since the laptop went
/// to sleep is not connectivity, and claiming otherwise is the kind of lie a
/// sync product does not recover from.
fn info_for(peer: &clipse_crypto::PairedDevice, state: Option<&PeerState>) -> PeerInfo {
    let last_seen_ms = state.and_then(|s| s.last_seen_ms);
    let fresh = last_seen_ms.is_some_and(|seen| now_ms().saturating_sub(seen) < ONLINE_WINDOW_MS);
    let connectivity = match (fresh, state.and_then(|s| s.last_reachability)) {
        (true, Some(Reachability::Tailnet)) => Connectivity::Tailnet,
        (true, _) => Connectivity::Lan,
        (false, _) => Connectivity::Offline,
    };

    PeerInfo {
        device: peer.device_id,
        label: peer.label.clone(),
        platform: platform_label(&peer.platform).to_string(),
        connectivity,
        last_seen_ms,
    }
}

fn platform_label(platform: &clipse_crypto::Platform) -> &str {
    match platform {
        clipse_crypto::Platform::Windows => "windows",
        clipse_crypto::Platform::MacOs => "macos",
        clipse_crypto::Platform::Linux => "linux",
        clipse_crypto::Platform::Other(other) => other,
    }
}

fn candidates_of(addresses: &[CandidateAddress]) -> CandidateList {
    CandidateList::new(addresses.iter().map(|address| match address {
        CandidateAddress::Lan(addr) => Candidate::lan(*addr),
        CandidateAddress::Tailnet(addr) => Candidate::tailnet(*addr),
    }))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use clipse_crypto::{DeviceIdentity, PairedDevice, Platform};
    use clipse_store::{Store, StoreOptions};
    use clipse_sync::LoopGuard;

    use clipse_net::DialError;

    use super::*;

    fn manager_with(peers: Vec<PairedDevice>) -> Arc<PeerManager> {
        let dir = tempfile::tempdir().unwrap();
        let paths = clipse_core::Paths::with_root(dir.path());
        let store = Arc::new(Store::open(&paths, StoreOptions::default()).unwrap());
        let device = DeviceId::generate();
        let identity = Arc::new(DeviceIdentity::generate(device));

        let trust = Arc::new(RwLock::new(Trust::new(device)));
        for peer in peers {
            trust.write().unwrap().add_peer(peer);
        }

        let transport = Arc::new(
            QuicTransport::bind(
                "127.0.0.1:0".parse().unwrap(),
                Arc::clone(&identity),
                Arc::clone(&trust),
            )
            .unwrap(),
        );

        let ctx = Arc::new(SyncContext {
            store,
            clock: Arc::new(clipse_core::HlcClock::new(device)),
            loop_guard: Arc::new(Mutex::new(LoopGuard::default())),
            label: "test".into(),
            platform: "test".into(),
            events: None,
        });

        // The temp dir must outlive the store; leaked deliberately, this is a
        // unit test that does not touch the filesystem again.
        std::mem::forget(dir);
        PeerManager::new(transport, ctx, trust)
    }

    fn paired(addresses: Vec<CandidateAddress>) -> PairedDevice {
        PairedDevice {
            device_id: DeviceId::generate(),
            static_public: DeviceIdentity::generate(DeviceId::generate()).public_key(),
            label: "peer".into(),
            platform: Platform::Linux,
            addresses,
            paired_at_ms: 0,
        }
    }

    #[tokio::test]
    async fn the_peer_table_mirrors_the_paired_set() {
        let a = paired(vec![CandidateAddress::Lan("127.0.0.1:1".parse().unwrap())]);
        let b = paired(vec![]);
        let manager = manager_with(vec![a.clone(), b.clone()]);

        assert_eq!(manager.counts(), (2, 2));

        manager
            .trust
            .write()
            .unwrap()
            .remove_peer(&b.device_id)
            .unwrap();
        manager.reload_from_trust();
        assert_eq!(manager.counts(), (1, 1), "a removed device must be dropped");
    }

    #[tokio::test]
    async fn pairing_addresses_become_dial_candidates_in_the_right_order() {
        let peer = paired(vec![
            CandidateAddress::Tailnet("100.64.0.1:7420".parse().unwrap()),
            CandidateAddress::Lan("192.168.1.5:7420".parse().unwrap()),
        ]);
        let manager = manager_with(vec![peer.clone()]);

        let peers = manager.peers.lock().unwrap();
        let order = peers.get(&peer.device_id).unwrap().candidates.dial_order();
        assert_eq!(order.len(), 2);
        assert_eq!(
            order[0].reachability,
            clipse_net::Reachability::Lan,
            "LAN must be tried before the tailnet"
        );
    }

    #[tokio::test]
    async fn an_unreachable_peer_is_retried_but_a_refusing_one_is_not() {
        // Nothing is listening on this address, so the dial fails as
        // unreachable — retryable, and the peer stays in the rotation.
        let peer = paired(vec![CandidateAddress::Lan("127.0.0.1:9".parse().unwrap())]);
        let manager = manager_with(vec![peer.clone()]);

        assert!(manager.sync_one(peer.device_id).await.is_err());
        assert!(
            manager.refusal(&peer.device_id).is_none(),
            "unreachable is not refusal"
        );
        assert_eq!(manager.counts(), (1, 1), "and it stays due for another try");
    }

    #[tokio::test]
    async fn dialling_an_unpaired_device_marks_it_refused_rather_than_looping() {
        let manager = manager_with(vec![]);
        let stranger = DeviceId::generate();

        // Not in the table at all.
        assert!(manager.sync_one(stranger).await.is_err());
        assert_eq!(manager.counts(), (0, 0));
    }

    // A tokio test because binding a QUIC endpoint needs a runtime.
    #[tokio::test]
    async fn a_refused_peer_is_given_another_chance_when_it_is_re_paired() {
        let peer = paired(vec![CandidateAddress::Lan("127.0.0.1:9".parse().unwrap())]);
        let manager = manager_with(vec![peer.clone()]);

        manager
            .peers
            .lock()
            .unwrap()
            .get_mut(&peer.device_id)
            .unwrap()
            .refused = Some("removed".into());
        assert_eq!(manager.counts(), (0, 1));

        manager.reload_from_trust();
        assert_eq!(
            manager.counts(),
            (1, 1),
            "re-pairing must clear an old refusal"
        );
    }

    #[test]
    fn dial_error_classification_is_what_drives_the_table() {
        // Guards the assumption the loops above are built on.
        assert!(
            DialError::Unreachable { attempts: vec![] }.is_retryable(),
            "asleep laptops must keep being tried"
        );
        assert!(!DialError::NotPaired.is_retryable());
    }

    /// The regression that made the product feel broken: sync only ever ran on
    /// a 30-second tick, so a copy took up to half a minute to appear on the
    /// other machine. A nudge has to start a pass now.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_nudge_starts_a_pass_instead_of_waiting_for_the_tick() {
        // No peers on purpose: what is under test is how quickly the loop
        // starts a pass, not how long dialling a dead address takes.
        let manager = manager_with(vec![]);
        let (shutdown, rx) = watch::channel(false);

        let looping = tokio::spawn(Arc::clone(&manager).dial_loop(rx));
        wait_until(&manager, 1)
            .await
            .expect("a daemon syncs on startup");

        manager.nudge();
        let woke = wait_until(&manager, 2).await;

        let _ = shutdown.send(true);
        looping.abort();
        assert!(
            woke.is_some(),
            "a nudge did not wake the loop; the tick is {DIAL_TICK:?} away"
        );
    }

    /// Poll for a pass count, well inside `DIAL_TICK` so a pass that only the
    /// timer could have caused cannot make this pass.
    async fn wait_until(manager: &Arc<PeerManager>, passes: u64) -> Option<()> {
        tokio::time::timeout(Duration::from_secs(2), async {
            while manager.passes() < passes {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .ok()
    }
}
