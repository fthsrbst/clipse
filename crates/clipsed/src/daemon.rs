//! The daemon's request handler and shared state.
//!
//! Capture, storage and sync are wired in as their crates land; what is here
//! is the part that does not depend on them — identity, settings, status and
//! the event sink the rest of the daemon publishes on.

use std::sync::{Mutex, OnceLock};

use clipse_core::Paths;
use clipse_ipc::protocol::{
    CaptureMode, DaemonStatus, ErrorCode, Event, IpcError, Request, Response, Settings,
};
use tokio::sync::broadcast;
use tracing::warn;

use crate::config::Config;
use crate::ipc_server::RequestHandler;

pub struct Daemon {
    paths: Paths,
    state: Mutex<State>,
    /// Set once, immediately after the IPC server is built. `OnceLock` rather
    /// than a constructor argument because the server needs the daemon first.
    events: OnceLock<broadcast::Sender<Event>>,
}

struct State {
    config: Config,
    paused: bool,
    capture_mode: CaptureMode,
}

impl Daemon {
    pub fn new(paths: Paths, config: Config) -> Self {
        Self {
            paths,
            state: Mutex::new(State {
                config,
                paused: false,
                capture_mode: CaptureMode::Automatic,
            }),
            events: OnceLock::new(),
        }
    }

    pub fn set_event_sink(&self, sink: broadcast::Sender<Event>) {
        let _ = self.events.set(sink);
    }

    /// Publish to subscribed UIs. Silently ignored when nobody is listening —
    /// the daemon runs happily with no UI open at all.
    pub fn emit(&self, event: Event) {
        if let Some(sink) = self.events.get() {
            let _ = sink.send(event);
        }
    }

    pub fn persist(&self) -> anyhow::Result<()> {
        let state = self.state.lock().expect("daemon state poisoned");
        state.config.save(&self.paths)?;
        Ok(())
    }

    fn status(&self) -> DaemonStatus {
        let state = self.state.lock().expect("daemon state poisoned");
        DaemonStatus {
            device: state.config.device,
            device_label: state.config.settings.device_label.clone(),
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            paused: state.paused,
            capture_mode: state.capture_mode.clone(),
            // Filled in once clipse-store is wired up.
            clip_count: 0,
            blob_bytes: 0,
            blob_quota_bytes: state.config.settings.blob_quota_bytes,
            peers_online: 0,
            peers_total: 0,
        }
    }

    fn settings(&self) -> Settings {
        self.state.lock().expect("daemon state poisoned").config.settings.clone()
    }
}

fn not_yet(feature: &str) -> Response {
    Response::Error(IpcError::new(
        ErrorCode::Unsupported,
        format!("{feature} is not wired up in this build yet"),
    ))
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
                    device: self.state.lock().expect("daemon state poisoned").config.device,
                }
            }

            Request::Status => Response::Status(Box::new(self.status())),
            Request::GetSettings => Response::Settings(Box::new(self.settings())),
            Request::Devices => Response::Devices(Vec::new()),
            Request::Subscribe => Response::Ok,

            Request::SetPaused { paused } => {
                {
                    let mut state = self.state.lock().expect("daemon state poisoned");
                    state.paused = paused;
                }
                self.emit(Event::StatusChanged(Box::new(self.status())));
                Response::Ok
            }

            Request::UpdateSettings(settings) => {
                {
                    let mut state = self.state.lock().expect("daemon state poisoned");
                    state.config.settings = *settings;
                }
                match self.persist() {
                    Ok(()) => {
                        self.emit(Event::StatusChanged(Box::new(self.status())));
                        Response::Ok
                    }
                    Err(e) => Response::Error(IpcError::new(ErrorCode::Internal, e.to_string())),
                }
            }

            Request::History(_) | Request::Search { .. } | Request::GetClip { .. } => {
                not_yet("history")
            }
            Request::Apply { .. } | Request::Paste { .. } => not_yet("clipboard injection"),
            Request::SetPinned { .. } | Request::Delete { .. } => not_yet("history editing"),
            Request::ForgetDevice { .. } => not_yet("pairing"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daemon() -> (tempfile::TempDir, Daemon) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::with_root(dir.path());
        paths.create_all().unwrap();
        let config = Config::load_or_create(&paths).unwrap();
        (dir, Daemon::new(paths, config))
    }

    #[tokio::test]
    async fn hello_reports_our_ipc_version_and_device() {
        let (_dir, daemon) = daemon();
        let response = daemon
            .handle(Request::Hello { client: "test".into(), ipc_version: clipse_ipc::IPC_VERSION })
            .await;

        match response {
            Response::Hello { ipc_version, .. } => {
                assert_eq!(ipc_version, clipse_ipc::IPC_VERSION)
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn settings_updates_are_persisted_immediately() {
        let (dir, daemon) = daemon();
        let paths = Paths::with_root(dir.path());

        let mut settings = daemon.settings();
        settings.hotkey = "Alt+Space".into();
        assert!(matches!(
            daemon.handle(Request::UpdateSettings(Box::new(settings))).await,
            Response::Ok
        ));

        // A crash right after this must not lose the change.
        let reloaded = Config::load_or_create(&paths).unwrap();
        assert_eq!(reloaded.settings.hotkey, "Alt+Space");
    }

    #[tokio::test]
    async fn pausing_shows_up_in_status() {
        let (_dir, daemon) = daemon();
        daemon.handle(Request::SetPaused { paused: true }).await;

        match daemon.handle(Request::Status).await {
            Response::Status(s) => assert!(s.paused),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn unimplemented_requests_say_so_rather_than_lying() {
        let (_dir, daemon) = daemon();
        match daemon.handle(Request::History(Default::default())).await {
            Response::Error(e) => assert_eq!(e.code, ErrorCode::Unsupported),
            other => panic!("history should not pretend to work yet: {other:?}"),
        }
    }
}
