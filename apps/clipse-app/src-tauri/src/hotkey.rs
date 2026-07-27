//! Registers the global hotkey that opens the popup, and re-registers it
//! whenever settings change.

use tauri::AppHandle;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::state::AppState;

pub fn register_initial(app: &AppHandle, hotkey: &str) {
    if let Err(e) = app.global_shortcut().register(hotkey) {
        tracing::warn!("could not register default hotkey {hotkey}: {e}");
    }
}

/// Swap the registered shortcut for `new_hotkey`, unregistering the previous
/// one first. A no-op when the hotkey did not actually change, so redundant
/// settings saves do not thrash the OS registration.
pub fn reregister(app: &AppHandle, state: &AppState, new_hotkey: &str) -> anyhow::Result<()> {
    let mut current = state.current_hotkey.lock().expect("hotkey mutex poisoned");
    if current.as_str() == new_hotkey {
        return Ok(());
    }

    let shortcuts = app.global_shortcut();
    if !current.is_empty() {
        // A malformed previous accelerator would already have failed to
        // register, so there is nothing meaningful to unregister — ignore.
        let _ = shortcuts.unregister(current.as_str());
    }
    shortcuts.register(new_hotkey)?;
    *current = new_hotkey.to_string();
    Ok(())
}
