//! Keeps the OS login item in step with the `start_at_login` setting.
//!
//! The setting has existed in `Settings` since the beginning and never did
//! anything: nothing read it, so turning it off changed a stored boolean and
//! nothing else. This is the piece that was missing.
//!
//! The daemon owns the preference — it is synced and persisted like every other
//! setting — while the operating system owns the registration. They can drift
//! (a user removes the login item by hand, or the setting arrives from another
//! device), so the two are reconciled on every launch and after every change,
//! with the setting treated as the intent.

use std::sync::Arc;

use clipse_ipc::{Request, Response};
use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;
use tracing::{debug, warn};

use crate::state::AppState;

/// Read the setting and make the OS agree with it.
pub fn apply_from_settings(app: &AppHandle, state: Arc<AppState>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let wanted = {
            let mut guard = state.client.lock().await;
            let Some(client) = guard.as_mut() else {
                // No daemon yet. `connection.rs` calls back here once the first
                // status arrives, so this is a deferral rather than a failure.
                return;
            };
            match client.call(Request::GetSettings).await {
                Ok(Response::Settings(settings)) => settings.start_at_login,
                _ => return,
            }
        };
        apply(&app, wanted);
    });
}

/// Register or unregister the login item.
///
/// Failures are logged, not surfaced: this is a convenience, and a locked-down
/// machine that refuses the registration is not a reason to interrupt someone.
pub fn apply(app: &AppHandle, enabled: bool) {
    let manager = app.autolaunch();
    let current = manager.is_enabled().unwrap_or(false);
    if current == enabled {
        return;
    }

    let outcome = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };

    match outcome {
        Ok(()) => debug!(enabled, "updated the login item"),
        Err(e) => warn!(error = %e, enabled, "could not update the login item"),
    }
}
