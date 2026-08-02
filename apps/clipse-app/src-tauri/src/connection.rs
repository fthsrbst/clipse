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
use clipse_ipc::{Client, DaemonStatus, Event, PeerInfo, Request, Response};
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
    // The login item is reconciled once, on the first connection of the
    // session. Doing it on every reconnect would fight a user who is toggling
    // the OS login item by hand while the daemon flaps.
    let mut reconciled_autostart = false;

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

                    if !reconciled_autostart {
                        reconciled_autostart = true;
                        crate::autostart::apply_from_settings(&app, Arc::clone(&state));
                    }
                }

                // The tray lists the paired devices, and nothing pushes that
                // list on connect — a `DeviceChanged` only arrives when one of
                // them changes, which for a device that has been offline all
                // day is never.
                forward_devices(&app, fetch_devices(&state).await);

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

    // An empty history has nothing to decorate the notch with, and a panel
    // showing three blank rows is worse than no panel: it retracts instead.
    if clips.iter().all(|clip| clip.deleted) {
        notch.hide().await;
    } else {
        notch.show(&clips, local_device).await;
    }
}

async fn fetch_devices(state: &AppState) -> Vec<PeerInfo> {
    let mut guard = state.client.lock().await;
    let Some(client) = guard.as_mut() else {
        return Vec::new();
    };
    match client.call(Request::Devices).await {
        Ok(Response::Devices(devices)) => devices,
        _ => Vec::new(),
    }
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

fn forward_devices(app: &AppHandle, devices: Vec<PeerInfo>) {
    let _ = app.emit("devices-changed", &devices);
    tray::on_devices(app, devices);
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
            let _ = app.emit("device-changed", &peer);
            tray::on_device_changed(app, peer);
        }
        Event::Suppressed { reason } => {
            let _ = app.emit("suppressed", reason);
        }
        // The offering device has no other way to learn that the ceremony it
        // put six digits on screen for actually finished.
        Event::PairingSucceeded { peer_label } => {
            let _ = app.emit("pairing-succeeded", peer_label);
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
