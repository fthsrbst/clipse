//! The daemon's request handler and shared state.
//!
//! Owns the history, the clipboard and the device identity. The UI reaches all
//! of it through `clipse-ipc` and nothing else; there is no path from a user
//! interface to the store or (in F2) to the network that does not come through
//! here.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use clipse_clipboard::{Clipboard, WatchMode, Watcher};
use clipse_core::{Clip, ClipFormat, ClipId, ClipSource, HlcClock, Paths};
use clipse_ipc::protocol::{
    CaptureMode, DaemonStatus, ErrorCode, Event, HistoryQuery, IpcError, Request, Response,
    Settings,
};
use clipse_store::Store;
use tokio::sync::broadcast;
use tracing::warn;

use crate::config::Config;
use crate::ipc_server::RequestHandler;

pub struct Daemon {
    paths: Paths,
    store: Arc<Store>,
    clock: Arc<HlcClock>,
    /// The watcher is also the `Clipboard` implementation, and it must stay
    /// alive for the whole run: dropping it stops the platform watch loop.
    watcher: Arc<Watcher>,
    state: Mutex<State>,
    /// Set once, immediately after the IPC server is built. `OnceLock` rather
    /// than a constructor argument because the server needs the daemon first.
    events: OnceLock<broadcast::Sender<Event>>,
    /// Absent when sync is disabled or the QUIC endpoint could not bind, in
    /// which case the status simply reports no peers rather than lying.
    peers: OnceLock<Arc<crate::peers::PeerManager>>,
    /// Everything the pairing requests need. Absent when sync is off, in which
    /// case pairing is refused rather than half-attempted.
    pairing: OnceLock<PairingContext>,
    /// Process-lifetime only; see `DaemonStatus::secrets_refused`.
    secrets_refused: AtomicU64,
}

/// What the pairing requests need in order to do anything.
pub struct PairingContext {
    pub identity: Arc<clipse_crypto::DeviceIdentity>,
    /// The single owner of the trust set, shared with the QUIC transport, so a
    /// new pairing takes effect for connections immediately.
    pub trust: Arc<std::sync::RwLock<clipse_crypto::Trust>>,
    /// A tokio mutex, not a std one: the ceremony is held across a network
    /// round trip, and a std guard across an await would make the request
    /// handler's future non-Send.
    pub state: Arc<tokio::sync::Mutex<crate::pairing::PairingState>>,
    pub peers: Arc<crate::peers::PeerManager>,
    /// Where a peer should dial us. Resolved once at startup.
    pub addresses: Vec<clipse_crypto::CandidateAddress>,
}

struct State {
    config: Config,
    paused: bool,
}

impl Daemon {
    pub fn new(
        paths: Paths,
        config: Config,
        store: Arc<Store>,
        watcher: Arc<Watcher>,
        clock: Arc<HlcClock>,
    ) -> Self {
        Self {
            paths,
            store,
            clock,
            watcher,
            state: Mutex::new(State {
                config,
                paused: false,
            }),
            events: OnceLock::new(),
            peers: OnceLock::new(),
            pairing: OnceLock::new(),
            secrets_refused: AtomicU64::new(0),
        }
    }

    /// Records that a capture was dropped for looking like a secret.
    ///
    /// The reason is logged and emitted; only the count is kept, and only in
    /// memory. Persisting it would mean writing a record about the thing that
    /// was refused, which is the one thing this product promises never to do.
    pub fn note_suppression(&self) {
        self.secrets_refused.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_pairing(&self, context: PairingContext) {
        let _ = self.pairing.set(context);
    }

    pub fn set_peers(&self, peers: Arc<crate::peers::PeerManager>) {
        let _ = self.peers.set(peers);
    }

    pub fn set_event_sink(&self, sink: broadcast::Sender<Event>) {
        let _ = self.events.set(sink);
    }

    /// Publish to subscribed UIs. Silently ignored when nobody is listening —
    /// the daemon runs happily with no UI open at all, which is the point of
    /// it being a separate process.
    pub fn emit(&self, event: Event) {
        if let Some(sink) = self.events.get() {
            let _ = sink.send(event);
        }
    }

    pub fn emit_status(&self) {
        if self.events.get().is_some() {
            let status = self.status();
            self.emit(Event::StatusChanged(Box::new(status)));
        }
    }

    pub fn store(&self) -> Arc<Store> {
        Arc::clone(&self.store)
    }

    pub fn clock(&self) -> &HlcClock {
        &self.clock
    }

    pub fn is_paused(&self) -> bool {
        self.state.lock().expect(POISONED).paused
    }

    pub fn clip_source(&self, app: Option<String>) -> ClipSource {
        let state = self.state.lock().expect(POISONED);
        ClipSource::new(
            state.config.device,
            state.config.settings.device_label.clone(),
        )
        .with_app(app)
    }

    pub fn persist(&self) -> anyhow::Result<()> {
        let state = self.state.lock().expect(POISONED);
        state.config.save(&self.paths)?;
        Ok(())
    }

    pub async fn load_clip(&self, id: ClipId) -> anyhow::Result<Option<Clip>> {
        let store = self.store();
        Ok(tokio::task::spawn_blocking(move || store.get(id)).await??)
    }

    /// Run a blocking store call off the async runtime. Every `Store` method is
    /// synchronous by design — see that crate's docs on why.
    async fn with_store<T, F>(&self, f: F) -> Result<T, Response>
    where
        F: FnOnce(&Store) -> clipse_store::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let store = self.store();
        match tokio::task::spawn_blocking(move || f(&store)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(Response::Error(IpcError::new(
                ErrorCode::Internal,
                e.to_string(),
            ))),
            Err(e) => Err(Response::Error(IpcError::new(
                ErrorCode::Internal,
                format!("store task failed: {e}"),
            ))),
        }
    }

    fn status(&self) -> DaemonStatus {
        let (device, device_label, paused, quota) = {
            let state = self.state.lock().expect(POISONED);
            (
                state.config.device,
                state.config.settings.device_label.clone(),
                state.paused,
                state.config.settings.blob_quota_bytes,
            )
        };

        // Counting rows is cheap and this is only asked for on demand or after
        // a write, so it is not worth caching and risking a stale readout.
        let clip_count = self.store.clip_count().unwrap_or(0);
        let blob_bytes = self.store.blob_bytes().unwrap_or(0);
        let (online, total) = self
            .peers
            .get()
            .map(|peers| peers.counts())
            .unwrap_or((0, 0));

        DaemonStatus {
            device,
            device_label,
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            paused,
            capture_mode: match self.watcher.mode() {
                WatchMode::Automatic => CaptureMode::Automatic,
                WatchMode::ManualPush { reason } => CaptureMode::ManualPush {
                    reason: reason.clone(),
                },
            },
            clip_count,
            blob_bytes,
            blob_quota_bytes: quota,
            peers_online: online,
            peers_total: total,
            secrets_refused: self.secrets_refused.load(Ordering::Relaxed),
        }
    }

    fn settings(&self) -> Settings {
        self.state.lock().expect(POISONED).config.settings.clone()
    }

    fn pairing_context(&self) -> Result<&PairingContext, Response> {
        self.pairing.get().ok_or_else(|| {
            Response::Error(IpcError::new(
                ErrorCode::Unsupported,
                "sync is disabled, so there is nothing to pair with",
            ))
        })
    }

    async fn begin_pairing(&self) -> Response {
        let context = match self.pairing_context() {
            Ok(context) => context,
            Err(response) => return response,
        };
        let label = self.settings().device_label;

        let mut state = context.state.lock().await;
        state.expire_if_stale();
        match state.begin(&context.identity, label, context.addresses.clone()) {
            Ok((uri, expires_at_ms)) => Response::PairingOffer { uri, expires_at_ms },
            Err(e) => Response::Error(IpcError::new(ErrorCode::BadRequest, e.to_string())),
        }
    }

    async fn pair_with_uri(&self, uri: &str) -> Response {
        let context = match self.pairing_context() {
            Ok(context) => context,
            Err(response) => return response,
        };
        let label = self.settings().device_label;
        let peers = Arc::clone(&context.peers);

        let outcome = {
            // Held across the network round trip on purpose: a second
            // PairWithUri arriving mid-ceremony must be refused, not
            // interleaved with this one.
            let mut state = context.state.lock().await;
            state
                .answer_offer(
                    uri,
                    &context.identity,
                    label,
                    context.addresses.clone(),
                    |addresses, accept| async move {
                        let addr = addresses
                            .iter()
                            .map(|address| match address {
                                clipse_crypto::CandidateAddress::Lan(addr)
                                | clipse_crypto::CandidateAddress::Tailnet(addr) => *addr,
                            })
                            .next()
                            .ok_or_else(|| "that code carries no address".to_string())?;
                        peers.send_pairing_accept(addr, &accept).await
                    },
                )
                .await
                .map(|()| state.digits())
        };

        match outcome {
            Ok(Some((digits, peer_label))) => Response::PairingCode { digits, peer_label },
            Ok(None) => Response::Error(IpcError::new(
                ErrorCode::Internal,
                "pairing produced no code",
            )),
            Err(e) => Response::Error(IpcError::new(ErrorCode::BadRequest, e.to_string())),
        }
    }

    /// The user compared the digits. This is the only place a device is ever
    /// trusted, and it happens only because a human said the codes matched.
    async fn confirm_pairing(&self, accept: bool) -> Response {
        let context = match self.pairing_context() {
            Ok(context) => context,
            Err(response) => return response,
        };

        if !accept {
            context.state.lock().await.cancel();
            self.emit(Event::PairingEnded {
                reason: "the codes did not match".into(),
            });
            return Response::Ok;
        }

        let peer = match context.state.lock().await.confirm() {
            Ok(peer) => peer,
            Err(e) => return Response::Error(IpcError::new(ErrorCode::BadRequest, e.to_string())),
        };

        {
            let mut trust = context.trust.write().expect(POISONED);
            trust.add_peer(peer);
            // Written while the lock is held: a crash between the in-memory
            // add and the file write would leave a device paired until the
            // next restart and then silently not.
            if let Err(e) = crate::identity::save_parts(&self.paths, &context.identity, &trust) {
                return Response::Error(IpcError::new(ErrorCode::Internal, e.to_string()));
            }
        }

        context.peers.reload_from_trust();
        self.emit_status();
        Response::Ok
    }

    fn forget_device(&self, device: clipse_core::DeviceId) -> Response {
        let context = match self.pairing_context() {
            Ok(context) => context,
            Err(response) => return response,
        };

        {
            let mut trust = context.trust.write().expect(POISONED);
            // Bumps the trust epoch, which is what stops the removed device's
            // existing sessions from authorising.
            if let Err(e) = trust.remove_peer(&device) {
                return Response::Error(IpcError::new(ErrorCode::NotFound, e.to_string()));
            }
            if let Err(e) = crate::identity::save_parts(&self.paths, &context.identity, &trust) {
                return Response::Error(IpcError::new(ErrorCode::Internal, e.to_string()));
            }
        }

        context.peers.reload_from_trust();
        self.emit_status();
        Response::Ok
    }

    /// Put a stored clip back on the clipboard, blobs and all.
    async fn apply(&self, id: ClipId) -> Response {
        let payloads = match self
            .with_store(move |store| {
                let Some(clip) = store.get(id)? else {
                    return Ok(None);
                };
                materialize(store, &clip).map(Some)
            })
            .await
        {
            Ok(Some(payloads)) => payloads,
            Ok(None) => {
                return Response::Error(IpcError::new(ErrorCode::NotFound, "no such clip"));
            }
            Err(response) => return response,
        };

        let watcher = Arc::clone(&self.watcher);
        match tokio::task::spawn_blocking(move || watcher.write(&payloads)).await {
            Ok(Ok(())) => Response::Ok,
            Ok(Err(e)) => Response::Error(IpcError::new(ErrorCode::Internal, e.to_string())),
            Err(e) => Response::Error(IpcError::new(
                ErrorCode::Internal,
                format!("clipboard task failed: {e}"),
            )),
        }
    }
}

const POISONED: &str = "daemon state poisoned by an earlier panic";

/// Gather every representation's bytes, pulling blobs off disk as needed.
///
/// A clip whose blob was evicted by the quota cannot be pasted in full; the
/// representations that survive are still written, because a paste that keeps
/// the text is far better than one that fails.
fn materialize(store: &Store, clip: &Clip) -> clipse_store::Result<Vec<(ClipFormat, Vec<u8>)>> {
    let mut out = Vec::with_capacity(clip.payloads.len());
    for payload in &clip.payloads {
        match payload.inline_bytes() {
            Some(bytes) => out.push((payload.format.clone(), bytes.to_vec())),
            None => match store.read_blob(&payload.digest) {
                Ok(bytes) => out.push((payload.format.clone(), bytes)),
                Err(e) => warn!(
                    format = payload.format.label(),
                    error = %e,
                    "blob missing while applying a clip; that representation is skipped"
                ),
            },
        }
    }
    Ok(out)
}

fn to_store_query(query: HistoryQuery) -> clipse_store::HistoryQuery {
    clipse_store::HistoryQuery {
        limit: query.limit as usize,
        offset: query.offset as usize,
        kind: query.kind,
        pinned_only: query.pinned_only,
    }
}

#[async_trait::async_trait]
impl RequestHandler for Daemon {
    async fn handle(&self, request: Request) -> Response {
        match request {
            Request::Hello { ipc_version, .. } => {
                if ipc_version != clipse_ipc::IPC_VERSION {
                    warn!(client = ipc_version, "ipc version mismatch");
                }
                Response::Hello {
                    daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                    ipc_version: clipse_ipc::IPC_VERSION,
                    device: self.state.lock().expect(POISONED).config.device,
                }
            }

            Request::Status => Response::Status(Box::new(self.status())),
            Request::GetSettings => Response::Settings(Box::new(self.settings())),
            Request::Devices => Response::Devices(Vec::new()),
            Request::Subscribe => Response::Ok,

            Request::History(query) => {
                let query = to_store_query(query);
                match self.with_store(move |store| store.recent(query)).await {
                    Ok(clips) => Response::Clips(clips),
                    Err(response) => response,
                }
            }

            Request::Search { text, query } => {
                let query = to_store_query(query);
                match self
                    .with_store(move |store| store.search(&text, query))
                    .await
                {
                    Ok(clips) => Response::Clips(clips),
                    Err(response) => response,
                }
            }

            Request::GetClip { id } => match self.with_store(move |store| store.get(id)).await {
                Ok(clip) => Response::Clip(clip.map(Box::new)),
                Err(response) => response,
            },

            Request::Apply { id } => self.apply(id).await,

            Request::Paste { id } => {
                let applied = self.apply(id).await;
                if !matches!(applied, Response::Ok) {
                    return applied;
                }
                match tokio::task::spawn_blocking(crate::paste::press_paste_shortcut).await {
                    Ok(Ok(())) => Response::Ok,
                    Ok(Err(e)) => {
                        // The content *is* on the clipboard; only the
                        // keystroke failed, so say so rather than implying
                        // nothing happened.
                        Response::Error(IpcError::new(
                            ErrorCode::Internal,
                            format!("copied, but the paste keystroke failed: {e}"),
                        ))
                    }
                    Err(e) => Response::Error(IpcError::new(
                        ErrorCode::Internal,
                        format!("paste task failed: {e}"),
                    )),
                }
            }

            Request::SetPinned { id, pinned } => {
                let hlc = self.clock.now();
                match self
                    .with_store(move |store| store.set_pinned(id, pinned, hlc))
                    .await
                {
                    Ok(()) => {
                        if let Ok(Some(clip)) = self.load_clip(id).await {
                            self.emit(Event::ClipUpdated(Box::new(clip)));
                        }
                        Response::Ok
                    }
                    Err(response) => response,
                }
            }

            Request::Delete { id } => {
                // A fresh HLC, so the tombstone is newer than the clip it
                // replaces and can replicate to the other devices.
                let hlc = self.clock.now();
                match self.with_store(move |store| store.delete(id, hlc)).await {
                    Ok(()) => {
                        self.emit(Event::ClipRemoved(id));
                        self.emit_status();
                        Response::Ok
                    }
                    Err(response) => response,
                }
            }

            Request::SetPaused { paused } => {
                self.state.lock().expect(POISONED).paused = paused;
                self.emit_status();
                Response::Ok
            }

            Request::UpdateSettings(settings) => {
                self.state.lock().expect(POISONED).config.settings = *settings;
                match self.persist() {
                    Ok(()) => {
                        self.emit_status();
                        Response::Ok
                    }
                    Err(e) => Response::Error(IpcError::new(ErrorCode::Internal, e.to_string())),
                }
            }

            Request::BeginPairing => self.begin_pairing().await,
            Request::PairWithUri { uri } => self.pair_with_uri(&uri).await,
            Request::ConfirmPairing { accept } => self.confirm_pairing(accept).await,
            Request::CancelPairing => {
                if let Some(context) = self.pairing.get() {
                    context.state.lock().await.cancel();
                }
                Response::Ok
            }
            Request::ForgetDevice { device } => self.forget_device(device),
        }
    }
}

#[cfg(test)]
mod tests {
    use clipse_core::{ClipFormat, DeviceId, INLINE_MAX_BYTES, Payload};
    use clipse_store::StoreOptions;

    use super::*;

    fn store_in(dir: &std::path::Path) -> Arc<Store> {
        let paths = Paths::with_root(dir);
        Arc::new(Store::open(&paths, StoreOptions::default()).unwrap())
    }

    fn text_clip(store: &Store, text: &str) -> Clip {
        let clock = HlcClock::new(DeviceId::generate());
        let clip = Clip::new(
            vec![Payload::new(ClipFormat::Text, text.as_bytes().to_vec())],
            ClipSource::new(DeviceId::generate(), "test"),
            clock.now(),
        );
        store.insert(&clip).unwrap();
        clip
    }

    #[test]
    fn materialize_returns_inline_bytes_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        let clip = text_clip(&store, "hello there");

        let payloads = materialize(&store, &clip).unwrap();
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].0, ClipFormat::Text);
        assert_eq!(payloads[0].1, b"hello there");
    }

    #[test]
    fn materialize_pulls_blob_bytes_back_off_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());

        let big = vec![9u8; (INLINE_MAX_BYTES + 1_000) as usize];
        let payload = Payload::new(ClipFormat::Png, big.clone());
        assert!(payload.is_blob(), "test premise");
        let digest = payload.digest;

        let clock = HlcClock::new(DeviceId::generate());
        let clip = Clip::new(
            vec![payload],
            ClipSource::new(DeviceId::generate(), "test"),
            clock.now(),
        );
        store.put_blob(&digest, &big).unwrap();
        store.insert(&clip).unwrap();

        let payloads = materialize(&store, &clip).unwrap();
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].1, big, "blob did not round-trip");
    }

    #[test]
    fn materialize_skips_an_evicted_blob_instead_of_failing() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());

        let big = vec![3u8; (INLINE_MAX_BYTES + 1_000) as usize];
        let clock = HlcClock::new(DeviceId::generate());
        let clip = Clip::new(
            vec![
                Payload::new(ClipFormat::Text, b"caption".to_vec()),
                Payload::new(ClipFormat::Png, big),
            ],
            ClipSource::new(DeviceId::generate(), "test"),
            clock.now(),
        );
        store.insert(&clip).unwrap();
        // Blob never written: this is the state a quota eviction leaves behind.

        let payloads = materialize(&store, &clip).unwrap();
        assert_eq!(payloads.len(), 1, "the text must still be pasteable");
        assert_eq!(payloads[0].0, ClipFormat::Text);
    }

    #[test]
    fn history_query_maps_across_the_ipc_boundary() {
        let query = to_store_query(HistoryQuery {
            limit: 25,
            offset: 50,
            kind: Some(clipse_core::ClipKind::Image),
            pinned_only: true,
        });
        assert_eq!(query.limit, 25);
        assert_eq!(query.offset, 50);
        assert_eq!(query.kind, Some(clipse_core::ClipKind::Image));
        assert!(query.pinned_only);
    }
}
