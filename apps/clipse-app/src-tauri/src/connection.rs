//! Connects to `clipsed`, keeps a command [`Client`] and a subscribed
//! [`EventStream`] alive, and reconnects with backoff whenever the daemon is
//! not running.
//!
//! Commands and events intentionally use two separate connections — mirrors
//! `clipse_ipc::Client`'s own split between `call` and `subscribe`, so a
//! long-lived event read never blocks a command the UI is waiting on.

use std::sync::Arc;
use std::time::Duration;

use clipse_ipc::client::EventStream;
use clipse_ipc::{Client, DaemonStatus, Event, Request, Response};
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

use crate::state::AppState;
use crate::tray;

const MIN_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// Spawn the reconnect-forever task. Runs for the lifetime of the app.
///
/// `ready` fires when the in-process daemon is answering requests. Waiting for
/// it means a fresh launch goes straight to the history instead of showing
/// "Clipse isn't running" for the half second before the first connect lands —
/// a lie, and the first thing a new user would see. A dropped sender resolves
/// immediately, so a daemon that failed to start still falls through to the
/// backoff loop and the honest offline state.
pub fn spawn(app: AppHandle, state: Arc<AppState>, ready: oneshot::Receiver<()>) {
    tauri::async_runtime::spawn(async move {
        let _ = ready.await;
        run_loop(app, state).await;
    });
}

async fn run_loop(app: AppHandle, state: Arc<AppState>) {
    let mut backoff = MIN_BACKOFF;

    loop {
        match connect_both(&state.endpoint).await {
            Ok((cmd_client, mut events)) => {
                {
                    let mut guard = state.client.lock().await;
                    *guard = Some(cmd_client);
                }
                emit_connection_state(&app, true);
                backoff = MIN_BACKOFF;

                // The notch needs to know which device is "here" so it can mark
                // clips that arrived from somewhere else; the daemon is the one
                // that knows, so it is read once per connection. It exists only
                // where the panel does — nothing else wants it, and a value
                // computed for nobody is a warning on every other platform.
                #[cfg(target_os = "macos")]
                let mut local_device = String::new();

                if let Some(status) = fetch_status(&state).await {
                    #[cfg(target_os = "macos")]
                    {
                        local_device = status.device.to_string();
                    }
                    forward_status(&app, &status);
                }

                #[cfg(target_os = "macos")]
                notch_refresh(&state, &local_device).await;

                while let Ok(event) = events.next().await {
                    // The panel shows the head of the history, so it is only
                    // worth redrawing when the head could have moved.
                    #[cfg(target_os = "macos")]
                    if matches!(
                        &event,
                        Event::ClipAdded(_) | Event::ClipRemoved(_) | Event::ClipUpdated(_)
                    ) {
                        notch_refresh(&state, &local_device).await;
                    }
                    forward_event(&app, event);
                }

                {
                    let mut guard = state.client.lock().await;
                    *guard = None;
                }
                emit_connection_state(&app, false);
            }
            Err(_) => emit_connection_state(&app, false),
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

async fn connect_both(endpoint: &str) -> anyhow::Result<(Client, EventStream)> {
    let cmd = Client::connect(endpoint, "clipse-app").await?;
    let evt = Client::connect(endpoint, "clipse-app-events").await?;
    let events = evt.subscribe().await?;
    Ok((cmd, events))
}

/// Push the current head of the history to the notch panel.
///
/// Read from the daemon rather than accumulated here: the panel shows the same
/// three clips the history window would, and keeping a second copy in this task
/// would be a second thing to get wrong about deletions and pins.
#[cfg(target_os = "macos")]
async fn notch_refresh(state: &Arc<AppState>, local_device: &str) {
    let Some(notch) = state.notch.get() else {
        return;
    };

    let clips = {
        let mut guard = state.client.lock().await;
        let Some(client) = guard.as_mut() else {
            return;
        };
        match client
            .call(Request::History(clipse_ipc::protocol::HistoryQuery::page(
                crate::notch::VISIBLE_CLIPS,
            )))
            .await
        {
            Ok(Response::Clips(clips)) => clips,
            _ => return,
        }
    };

    notch.show(&clips, local_device).await;
}

async fn fetch_status(state: &AppState) -> Option<DaemonStatus> {
    let mut guard = state.client.lock().await;
    let client = guard.as_mut()?;
    match client.call(Request::Status).await {
        Ok(Response::Status(status)) => Some(*status),
        _ => None,
    }
}

fn forward_status(app: &AppHandle, status: &DaemonStatus) {
    let _ = app.emit("status-changed", status);
    tray::on_status(app, status);
}

fn forward_event(app: &AppHandle, event: Event) {
    match event {
        Event::ClipAdded(clip) => {
            let _ = app.emit("clip-added", *clip);
        }
        Event::ClipUpdated(clip) => {
            let _ = app.emit("clip-updated", *clip);
        }
        Event::ClipRemoved(id) => {
            let _ = app.emit("clip-removed", id);
        }
        Event::StatusChanged(status) => forward_status(app, &status),
        Event::DeviceChanged(peer) => {
            let _ = app.emit("device-changed", peer);
        }
        Event::Suppressed { reason } => {
            let _ = app.emit("suppressed", reason);
        }
        // Both devices show these six digits and the user compares them, so
        // they go straight to the webview with no interpretation here.
        Event::PairingCode { digits, peer_label } => {
            let _ = app.emit("pairing-code", (digits, peer_label));
        }
        Event::PairingEnded { reason } => {
            let _ = app.emit("pairing-ended", reason);
        }
    }
}

fn emit_connection_state(app: &AppHandle, connected: bool) {
    let _ = app.emit("connection-changed", connected);
    if !connected {
        tray::on_disconnected(app);
    }
}
