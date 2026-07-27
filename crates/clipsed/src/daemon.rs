//! The daemon's request handler and shared state.
//!
//! Owns the history, the clipboard and the device identity. The UI reaches all
//! of it through `clipse-ipc` and nothing else; there is no path from a user
//! interface to the store or (in F2) to the network that does not come through
//! here.

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
        }
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
        }
    }

    fn settings(&self) -> Settings {
        self.state.lock().expect(POISONED).config.settings.clone()
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

            Request::ForgetDevice { .. } => Response::Error(IpcError::new(
                ErrorCode::Unsupported,
                "pairing arrives with peer sync",
            )),
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
