//! Tauri commands. Each one wraps a single `clipse_ipc::Request` and unwraps
//! the matching `Response` variant — the protocol is frozen, so a mismatch
//! here is a programmer error in this crate, not something callers recover
//! from.

use std::sync::Arc;

use clipse_core::{Clip, ClipId};
use clipse_ipc::client::ClientError;
use clipse_ipc::{DaemonStatus, HistoryQuery, PeerInfo, Request, Response, Settings};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::AppState;

/// Serializable across the invoke boundary, and specific enough that the
/// frontend can render "daemon not running" differently from a real error
/// the daemon reported.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandError {
    NotConnected,
    Daemon { code: String, message: String },
    // A struct variant, not a newtype: serde cannot serialize a tagged
    // newtype variant whose payload is a bare string (it requires the
    // payload to serialize as a map), so `Transport(String)` would fail at
    // the invoke boundary instead of reaching the frontend.
    Transport { message: String },
}

impl From<ClientError> for CommandError {
    fn from(err: ClientError) -> Self {
        match err {
            ClientError::Daemon(e) => Self::Daemon {
                code: e.code.to_string(),
                message: e.message,
            },
            other => Self::Transport {
                message: other.to_string(),
            },
        }
    }
}

async fn call(state: &AppState, request: Request) -> Result<Response, CommandError> {
    let mut guard = state.client.lock().await;
    let client = guard.as_mut().ok_or(CommandError::NotConnected)?;
    Ok(client.call(request).await?)
}

fn unexpected(what: &'static str) -> CommandError {
    CommandError::Transport {
        message: format!("daemon sent an unexpected response to {what}"),
    }
}

#[tauri::command]
pub async fn history(
    state: State<'_, Arc<AppState>>,
    query: HistoryQuery,
) -> Result<Vec<Clip>, CommandError> {
    match call(&state, Request::History(query)).await? {
        Response::Clips(clips) => Ok(clips),
        _ => Err(unexpected("history")),
    }
}

#[tauri::command]
pub async fn search(
    state: State<'_, Arc<AppState>>,
    text: String,
    query: HistoryQuery,
) -> Result<Vec<Clip>, CommandError> {
    match call(&state, Request::Search { text, query }).await? {
        Response::Clips(clips) => Ok(clips),
        _ => Err(unexpected("search")),
    }
}

#[tauri::command]
pub async fn get_clip(
    state: State<'_, Arc<AppState>>,
    id: ClipId,
) -> Result<Option<Clip>, CommandError> {
    match call(&state, Request::GetClip { id }).await? {
        Response::Clip(clip) => Ok(clip.map(|c| *c)),
        _ => Err(unexpected("get_clip")),
    }
}

#[tauri::command]
pub async fn apply(state: State<'_, Arc<AppState>>, id: ClipId) -> Result<(), CommandError> {
    match call(&state, Request::Apply { id }).await? {
        Response::Ok => Ok(()),
        _ => Err(unexpected("apply")),
    }
}

#[tauri::command]
pub async fn paste(state: State<'_, Arc<AppState>>, id: ClipId) -> Result<(), CommandError> {
    paste_from(&state, id).await
}

/// The same thing without a `tauri::State` wrapper, so callers that are not
/// commands — the notch sidecar bridge — can reach it.
pub async fn paste_from(state: &Arc<AppState>, id: ClipId) -> Result<(), CommandError> {
    match call(state, Request::Paste { id }).await? {
        Response::Ok => Ok(()),
        _ => Err(unexpected("paste")),
    }
}

#[tauri::command]
pub async fn set_pinned(
    state: State<'_, Arc<AppState>>,
    id: ClipId,
    pinned: bool,
) -> Result<(), CommandError> {
    match call(&state, Request::SetPinned { id, pinned }).await? {
        Response::Ok => Ok(()),
        _ => Err(unexpected("set_pinned")),
    }
}

#[tauri::command]
pub async fn delete(state: State<'_, Arc<AppState>>, id: ClipId) -> Result<(), CommandError> {
    match call(&state, Request::Delete { id }).await? {
        Response::Ok => Ok(()),
        _ => Err(unexpected("delete")),
    }
}

#[tauri::command]
pub async fn status(state: State<'_, Arc<AppState>>) -> Result<DaemonStatus, CommandError> {
    match call(&state, Request::Status).await? {
        Response::Status(status) => Ok(*status),
        _ => Err(unexpected("status")),
    }
}

#[tauri::command]
pub async fn set_paused(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    paused: bool,
) -> Result<(), CommandError> {
    match call(&state, Request::SetPaused { paused }).await? {
        Response::Ok => {
            refresh_tray_status(app, state.inner().clone());
            Ok(())
        }
        _ => Err(unexpected("set_paused")),
    }
}

#[tauri::command]
pub async fn devices(state: State<'_, Arc<AppState>>) -> Result<Vec<PeerInfo>, CommandError> {
    match call(&state, Request::Devices).await? {
        Response::Devices(devices) => Ok(devices),
        _ => Err(unexpected("devices")),
    }
}

/// What `BeginPairing` produces: the string the pairing screen renders as a QR
/// code, and when it stops being valid.
#[derive(serde::Serialize)]
pub struct PairingOffer {
    pub uri: String,
    pub expires_at_ms: u64,
    /// The same offer as a self-contained SVG, ready to drop into the DOM.
    /// `None` if the payload could not be encoded, in which case the screen
    /// falls back to the copyable string — which pairs devices identically.
    pub svg: Option<String>,
}

/// The six digits both devices show. The user compares them; matching is what
/// makes the pairing safe, so the screen must present them side by side and
/// ask, not merely display them.
#[derive(serde::Serialize)]
pub struct PairingCode {
    pub digits: String,
    pub peer_label: String,
}

#[tauri::command]
pub async fn begin_pairing(state: State<'_, Arc<AppState>>) -> Result<PairingOffer, CommandError> {
    match call(&state, Request::BeginPairing).await? {
        Response::PairingOffer { uri, expires_at_ms } => {
            let svg = render_qr(&uri);
            Ok(PairingOffer {
                uri,
                expires_at_ms,
                svg,
            })
        }
        _ => Err(unexpected("begin_pairing")),
    }
}

/// The offer as an SVG QR code.
///
/// Error correction is deliberately the lowest level the payload allows: the
/// code is read from a screen a foot away, not off a printed label in a
/// warehouse, and a lower level keeps the modules large enough to scan on a
/// small window.
fn render_qr(uri: &str) -> Option<String> {
    use qrcode::{EcLevel, QrCode, render::svg};

    let code = QrCode::with_error_correction_level(uri, EcLevel::L).ok()?;
    Some(
        code.render::<svg::Color<'_>>()
            .min_dimensions(200, 200)
            // currentColor so the code inherits the theme instead of being a
            // white rectangle on a dark background.
            .dark_color(svg::Color("currentColor"))
            .light_color(svg::Color("transparent"))
            .quiet_zone(true)
            .build(),
    )
}

#[tauri::command]
pub async fn pair_with_uri(
    state: State<'_, Arc<AppState>>,
    uri: String,
) -> Result<PairingCode, CommandError> {
    match call(&state, Request::PairWithUri { uri }).await? {
        Response::PairingCode { digits, peer_label } => Ok(PairingCode { digits, peer_label }),
        _ => Err(unexpected("pair_with_uri")),
    }
}

/// `accept` is the user's answer to "do these six digits match?". Passing true
/// without having asked would defeat the only defence against a
/// man-in-the-middle that this protocol has.
#[tauri::command]
pub async fn confirm_pairing(
    state: State<'_, Arc<AppState>>,
    accept: bool,
) -> Result<(), CommandError> {
    match call(&state, Request::ConfirmPairing { accept }).await? {
        Response::Ok => Ok(()),
        _ => Err(unexpected("confirm_pairing")),
    }
}

#[tauri::command]
pub async fn cancel_pairing(state: State<'_, Arc<AppState>>) -> Result<(), CommandError> {
    match call(&state, Request::CancelPairing).await? {
        Response::Ok => Ok(()),
        _ => Err(unexpected("cancel_pairing")),
    }
}

#[tauri::command]
pub async fn forget_device(
    state: State<'_, Arc<AppState>>,
    device: clipse_core::DeviceId,
) -> Result<(), CommandError> {
    match call(&state, Request::ForgetDevice { device }).await? {
        Response::Ok => Ok(()),
        _ => Err(unexpected("forget_device")),
    }
}

#[tauri::command]
pub async fn get_settings(state: State<'_, Arc<AppState>>) -> Result<Settings, CommandError> {
    match call(&state, Request::GetSettings).await? {
        Response::Settings(settings) => Ok(*settings),
        _ => Err(unexpected("get_settings")),
    }
}

#[tauri::command]
pub async fn update_settings(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    settings: Settings,
) -> Result<Settings, CommandError> {
    let applied = match call(&state, Request::UpdateSettings(Box::new(settings))).await? {
        Response::Settings(settings) => *settings,
        _ => return Err(unexpected("update_settings")),
    };

    if let Err(e) = crate::hotkey::reregister(&app, &state, &applied.hotkey) {
        tracing::warn!("could not re-register hotkey {}: {e}", applied.hotkey);
    }

    // Driven from what the daemon *applied*, not from what was asked for: the
    // daemon is the authority on settings, and the login item should follow the
    // stored value rather than an optimistic one.
    crate::autostart::apply(&app, applied.start_at_login);

    Ok(applied)
}

#[tauri::command]
pub fn hide_popup(app: AppHandle) {
    crate::popup::hide(&app);
}

/// Fired from the tray's Pause/Resume item, which has no invoke round-trip to
/// hang a `Result` off — errors are logged instead of surfaced.
pub fn toggle_pause_from_tray(app: AppHandle) {
    let state = app.state::<Arc<AppState>>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let current = match call(&state, Request::Status).await {
            Ok(Response::Status(status)) => status.paused,
            _ => return,
        };
        let _ = call(&state, Request::SetPaused { paused: !current }).await;
    });
}

fn refresh_tray_status(app: AppHandle, state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        if let Ok(Response::Status(status)) = call(&state, Request::Status).await {
            let _ = app.emit("status-changed", &*status);
            crate::tray::on_status(&app, &status);
        }
    });
}
