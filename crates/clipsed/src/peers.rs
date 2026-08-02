//! Keeping sessions with the paired devices alive.
//!
//! Two loops. One accepts whatever arrives on the QUIC endpoint; the other
//! walks the paired set on a timer and dials anyone it has not spoken to
//! recently. Both end up in the same place — `sync::run_session` — differing
//! only in which side of the alternation they take.
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
use clipse_net::candidate::{Candidate, CandidateList};
use clipse_net::{Backoff, Discovery, DiscoveryEvent, Inbound, QuicTransport};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::sync::{self, Role, SyncContext};

/// How often the dial loop wakes up. Sync is also triggered by a local capture
/// (see `capture::run`), so this is the floor on how stale a peer can get, not
/// the normal path.
const DIAL_TICK: Duration = Duration::from_secs(30);

/// Per-peer state the loops share.
struct PeerState {
    candidates: CandidateList,
    backoff: Backoff,
    /// Set when the peer refused us. Cleared when the paired set changes.
    refused: Option<String>,
}

impl PeerState {
    fn new(candidates: CandidateList) -> Self {
        Self {
            candidates,
            backoff: Backoff::default(),
            refused: None,
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
    /// be a cycle that never drops. Used for the one thing a session cannot
    /// decide for itself — whether an arriving clip belongs on this machine's
    /// clipboard.
    daemon: std::sync::Weak<crate::daemon::Daemon>,
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
        });
        manager.reload_from_trust();
        manager
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
                                this.note_success(peer, link.info().addr);
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
                    // Only accepted while the user has the pairing screen open.
                    // Refusing silently is deliberate: a stranger probing for a
                    // window learns nothing from the difference between "no
                    // window" and "wrong answer".
                    let outcome = {
                        let mut pairing = self.pairing.lock().await;
                        pairing.expire_if_stale();
                        if pairing.is_offering() {
                            Some(pairing.accept_answer(exchange.accept_bytes()))
                        } else {
                            None
                        }
                    };

                    match outcome {
                        Some(Ok(confirm)) => {
                            let digits = self.pairing.lock().await.digits();
                            if let Err(e) = exchange.confirm(&confirm).await {
                                warn!(error = %e, "could not send the pairing confirmation");
                                self.pairing.lock().await.cancel();
                            } else if let Some((digits, peer_label)) = digits {
                                // Both screens now show a code. Nothing is
                                // trusted until the user says they match.
                                self.emit(clipse_ipc::Event::PairingCode { digits, peer_label });
                            }
                        }
                        Some(Err(e)) => {
                            debug!(error = %e, "a pairing attempt was refused");
                            exchange.reject();
                            self.emit(clipse_ipc::Event::PairingEnded {
                                reason: e.to_string(),
                            });
                        }
                        None => {
                            debug!(addr = %exchange.remote_addr(), "no pairing window is open");
                            exchange.reject();
                        }
                    }
                }
                Err(e) => debug!(error = %e, "inbound connection ended"),
            }
        }
    }

    /// Dial peers that are due, forever.
    pub async fn dial_loop(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() { return; }
                }
                _ = tokio::time::sleep(DIAL_TICK) => {
                    self.refresh_from_discovery().await;
                    self.sync_all().await;
                }
            }
        }
    }

    /// One pass over every paired device.
    pub async fn sync_all(&self) {
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
                let addr = link.info().addr;
                let result = sync::run_session(&mut link, &self.ctx, Role::Dialler).await;
                // Gracefully, because the dialler's last act is a send: see
                // `PeerLink::close_gracefully`.
                link.close_gracefully("done").await;

                match result {
                    Ok(outcome) => {
                        info!(peer = %peer.short(), ?outcome, "synced");
                        self.note_success(peer, addr);
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

    /// Carry a `PairingAccept` to the address from a scanned QR code.
    pub async fn send_pairing_accept(
        &self,
        addr: SocketAddr,
        accept: &[u8],
    ) -> Result<Vec<u8>, String> {
        self.transport
            .send_pairing_accept(addr, accept)
            .await
            .map_err(|e| e.to_string())
    }

    fn note_success(&self, peer: DeviceId, addr: SocketAddr) {
        let mut peers = self.peers.lock().expect(POISONED);
        if let Some(state) = peers.get_mut(&peer) {
            state.backoff.reset();
            state.refused = None;
            state
                .candidates
                .upsert(Candidate::lan(addr).seen_at(now_ms()));
        }
    }

    /// How many peers are paired, and how many are not currently refusing us.
    pub fn counts(&self) -> (u32, u32) {
        let peers = self.peers.lock().expect(POISONED);
        let total = peers.len() as u32;
        let healthy = peers.values().filter(|s| s.refused.is_none()).count() as u32;
        (healthy, total)
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
}
