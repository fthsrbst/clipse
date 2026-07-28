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

                if let Some(status) = fetch_status(&state).await {
                    forward_status(&app, &status);
                }

                while let Ok(event) = events.next().await {
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
