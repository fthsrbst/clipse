mod commands;
mod connection;
mod hotkey;
mod popup;
mod state;
mod tray;

use std::sync::Arc;

use tauri::Manager;
use tauri_plugin_global_shortcut::ShortcutState;

use state::AppState;

/// `CLIPSE_DATA_DIR` lets a developer point both the app and the mock daemon
/// at the same temp directory (see `examples/mock-daemon.rs` and the README);
/// production installs fall back to the platform default.
fn resolve_endpoint() -> String {
    let paths = match std::env::var_os("CLIPSE_DATA_DIR") {
        Some(dir) => clipse_core::Paths::with_root(dir),
        None => clipse_core::Paths::platform_default()
            .unwrap_or_else(|_| clipse_core::Paths::with_root(".clipse-dev/app")),
    };
    let _ = paths.create_all();
    paths.ipc_endpoint()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let endpoint = resolve_endpoint();
    let default_hotkey = clipse_ipc::Settings::default().hotkey;
    let app_state = Arc::new(AppState::new(endpoint, default_hotkey.clone()));
    let connection_state = app_state.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        let _ = popup::show_near_cursor(app);
                    }
                })
                .build(),
        )
        .manage(app_state)
        .setup(move |app| {
            let handle = app.handle().clone();

            tray::build(&handle)?;
            hotkey::register_initial(&handle, &default_hotkey);
            connection::spawn(handle.clone(), connection_state);

            // Escape is handled in the popup's own JS (it calls the
            // `hide_popup` command); losing focus — clicking elsewhere — is
            // handled here so it works even if the webview never gets the key.
            if let Some(popup) = app.get_webview_window(popup::LABEL) {
                let hide_handle = handle.clone();
                popup.on_window_event(move |event| {
                    if let tauri::WindowEvent::Focused(false) = event {
                        popup::hide(&hide_handle);
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::history,
            commands::search,
            commands::get_clip,
            commands::apply,
            commands::paste,
            commands::set_pinned,
            commands::delete,
            commands::status,
            commands::set_paused,
            commands::devices,
            commands::get_settings,
            commands::update_settings,
            commands::hide_popup,
            commands::begin_pairing,
            commands::pair_with_uri,
            commands::confirm_pairing,
            commands::cancel_pairing,
            commands::forget_device,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
