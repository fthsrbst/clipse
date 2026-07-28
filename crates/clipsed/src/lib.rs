//! The Clipse daemon, as a library.
//!
//! Owns the clipboard, the history and the peer sessions. User interfaces are
//! clients over `clipse-ipc`.
//!
//! This is a library so the desktop app can ship as **one executable**: it
//! calls [`run`] on a background task instead of hunting for a second binary
//! to launch. That is a packaging decision, not an architectural one — the app
//! still reaches the daemon only through `clipse-ipc`, exactly as it would
//! across a process boundary, and the daemon is still the single source of
//! truth. Nothing here knows a window exists.
//!
//! Running it out of process is still supported and still useful: `clipsed`
//! keeps syncing after the last window closes. Whoever binds the endpoint
//! first wins, and the loser becomes a client — see [`Started`].

mod capture;
mod config;
mod daemon;
mod identity;
mod ipc_server;
mod pairing;
mod paste;
mod peers;
mod sync;

use std::future::Future;
use std::sync::Arc;

use anyhow::Context as _;
use clipse_clipboard::{WatchConfig, WatchMode, sensitive::AppBlocklist, watch};
use clipse_core::{HlcClock, Paths};
use clipse_ipc::transport::{Listener, TransportError};
use clipse_store::{Store, StoreOptions};
use tracing::{info, warn};

/// How [`run`] finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Started {
    /// This process was the daemon, and has now shut down.
    Daemon,
    /// Another process already owned the data directory, so this call did
    /// nothing. The caller should connect to that daemon as a client rather
    /// than treat it as a failure — it is the normal outcome when the app
    /// starts while a standalone `clipsed` is already running.
    AlreadyRunning,
}

/// Everything the daemon needs to know before it starts.
pub struct RunOptions {
    /// Where history, blobs and config live.
    pub paths: Paths,
    /// Signalled once the IPC endpoint is answering requests.
    ///
    /// The app uses this to hold its first frame until the daemon can actually
    /// reply, so a fresh install never flashes "Clipse isn't running" at
    /// someone who has just opened it for the first time.
    pub ready: Option<tokio::sync::oneshot::Sender<()>>,
}

impl RunOptions {
    pub fn new(paths: Paths) -> Self {
        Self { paths, ready: None }
    }

    pub fn with_ready(mut self, ready: tokio::sync::oneshot::Sender<()>) -> Self {
        self.ready = Some(ready);
        self
    }
}

/// Run the daemon until `shutdown` resolves.
pub async fn run(
    options: RunOptions,
    shutdown: impl Future<Output = ()> + Send,
) -> anyhow::Result<Started> {
    let RunOptions { paths, ready } = options;

    paths
        .create_all()
        .with_context(|| format!("creating {}", paths.root().display()))?;

    let config = config::Config::load_or_create(&paths)?;

    // Loaded at startup rather than lazily: if the key file is corrupt or
    // belongs to another device, the user needs to know now, not the first
    // time they try to pair.
    let identity::Identity {
        identity: device_key,
        trust: device_trust,
    } = identity::Identity::load_or_create(&paths, config.device)?;
    info!(
        device = %config.device.short(),
        fingerprint = %device_key.fingerprint(),
        peers = device_trust.peers().count(),
        root = %paths.root().display(),
        "starting clipsed"
    );

    // Bound before anything expensive: if another daemon already owns this
    // data directory, stop now rather than opening its database too.
    let listener = match Listener::bind(&paths.ipc_endpoint()).await {
        Ok(listener) => listener,
        Err(TransportError::AlreadyRunning { endpoint }) => {
            info!(endpoint, "a clipsed already owns this data directory");
            return Ok(Started::AlreadyRunning);
        }
        Err(e) => return Err(e).context("binding the IPC endpoint"),
    };
    info!(endpoint = listener.endpoint(), "ipc listening");

    let store = Arc::new(
        Store::open(
            &paths,
            StoreOptions::with_quota(config.settings.blob_quota_bytes),
        )
        .context("opening the history store")?,
    );

    let watch_config = WatchConfig {
        app_blocklist: AppBlocklist::defaults().with_extra(config.settings.blocked_apps.clone()),
        detect_secrets: config.settings.detect_secrets,
        ..WatchConfig::default()
    };
    let (watcher, captures) = watch(watch_config).context("starting the clipboard watcher")?;
    if let WatchMode::ManualPush { reason } = watcher.mode() {
        // Not an error: this is GNOME Wayland, and the user needs to be told
        // rather than left wondering why nothing is being captured.
        warn!(%reason, "automatic clipboard capture is unavailable on this desktop");
    }

    let clock = Arc::new(HlcClock::new(config.device));
    let sync_enabled = config.settings.sync_enabled;
    let announce = config.settings.announce_on_network;
    let device_label = config.settings.device_label.clone();
    let label_for_record = device_label.clone();
    let config_device = config.device;
    let device_fingerprint = device_key.fingerprint().to_string();
    let loop_guard = Arc::new(std::sync::Mutex::new(clipse_sync::LoopGuard::default()));
    let pairing_state = Arc::new(tokio::sync::Mutex::new(pairing::PairingState::default()));

    let daemon = Arc::new(daemon::Daemon::new(
        paths,
        config,
        Arc::clone(&store),
        Arc::new(watcher),
        Arc::clone(&clock),
    ));

    let server = ipc_server::IpcServer::new(Arc::clone(&daemon));
    let events = server.events();
    daemon.set_event_sink(events.clone());

    let capturing = tokio::spawn(capture::run(Arc::clone(&daemon), captures));

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Peer sync. Bound on any interface so a peer on the LAN can reach us;
    // port 0 because the port is advertised, not fixed.
    let peer_loops = if sync_enabled {
        let device_key = Arc::new(device_key);
        let trust = Arc::new(std::sync::RwLock::new(device_trust));
        match clipse_net::QuicTransport::bind(
            "0.0.0.0:0"
                .parse()
                .expect("a literal address always parses"),
            Arc::clone(&device_key),
            Arc::clone(&trust),
        ) {
            Ok(transport) => {
                let transport = Arc::new(transport);
                info!(addr = %transport.local_addr(), "quic listening");
                let ctx = Arc::new(sync::SyncContext {
                    store,
                    clock,
                    loop_guard,
                    label: device_label,
                    platform: std::env::consts::OS.to_string(),
                });
                let record = clipse_net::ServiceRecord::new(
                    config_device,
                    device_fingerprint,
                    label_for_record,
                    std::env::consts::OS,
                );
                let quic_port = transport.local_addr().port();
                let mut manager =
                    peers::PeerManager::new(Arc::clone(&transport), ctx, Arc::clone(&trust));
                if announce {
                    match clipse_net::Discovery::start(&record, quic_port) {
                        Ok(discovery) => {
                            info!("announcing on the local network");
                            manager = manager.with_discovery(discovery);
                        }
                        // Not fatal: peers recorded at pairing time are still
                        // reachable, they just will not be re-found automatically.
                        Err(e) => warn!(error = %e, "mDNS unavailable; discovery is off"),
                    }
                } else {
                    // The announcement is what tells the rest of the LAN that
                    // this machine runs Clipse. Someone who would rather not
                    // say so trades automatic re-discovery for silence.
                    info!(
                        "network announcement is off; peers are reached at their paired addresses"
                    );
                }
                let manager = manager.with_pairing(Arc::clone(&pairing_state), events);
                daemon.set_peers(Arc::clone(&manager));

                let addresses = reachable_addresses(quic_port);
                if addresses.is_empty() {
                    warn!("no reachable address found; a QR code would be undialable");
                }
                daemon.set_pairing(daemon::PairingContext {
                    identity: Arc::clone(&device_key),
                    trust,
                    state: Arc::clone(&pairing_state),
                    peers: Arc::clone(&manager),
                    addresses,
                });
                Some((
                    tokio::spawn(Arc::clone(&manager).accept_loop(shutdown_rx.clone())),
                    tokio::spawn(manager.dial_loop(shutdown_rx.clone())),
                ))
            }
            Err(e) => {
                // Not fatal: the local half of the product still works, and
                // the UI will show sync as unavailable rather than the daemon
                // refusing to start.
                warn!(error = %e, "could not start peer sync; running local-only");
                None
            }
        }
    } else {
        info!("sync is disabled in settings; running local-only");
        None
    };

    // Served last, on purpose. The listener is bound early so a second daemon
    // fails fast, but answering requests before pairing and sync are wired up
    // would mean a UI connecting at the wrong moment gets told sync is
    // disabled when it is merely not ready yet.
    let serving = tokio::spawn(server.serve(listener, shutdown_rx));
    if let Some(ready) = ready {
        let _ = ready.send(());
    }

    shutdown.await;
    info!("shutting down");
    let _ = shutdown_tx.send(true);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), serving).await;
    capturing.abort();
    if let Some((accepting, dialling)) = peer_loops {
        accepting.abort();
        dialling.abort();
    }

    daemon.persist()?;
    Ok(Started::Daemon)
}

/// Addresses a peer could dial us on, for the QR code.
///
/// The QUIC endpoint binds `0.0.0.0`, which is not something anyone can dial,
/// so the primary interface address is found by opening a UDP socket toward a
/// public address and asking which local address the routing table chose. No
/// packet is ever sent — this is a routing-table query wearing a socket.
fn reachable_addresses(port: u16) -> Vec<clipse_crypto::CandidateAddress> {
    use clipse_crypto::CandidateAddress;

    let mut addresses = Vec::new();

    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0")
        && socket.connect("198.51.100.1:9").is_ok()
        && let Ok(local) = socket.local_addr()
    {
        addresses.push(CandidateAddress::Lan(std::net::SocketAddr::new(
            local.ip(),
            port,
        )));
    }

    // Tailscale is optional; a machine without it is a LAN-only Clipse.
    if let Ok(status) = clipse_net::TailnetStatus::query()
        && let Some(ip) = status.this_device.and_then(|device| device.preferred_ip())
    {
        addresses.push(CandidateAddress::Tailnet(std::net::SocketAddr::new(
            ip, port,
        )));
    }

    addresses
}
