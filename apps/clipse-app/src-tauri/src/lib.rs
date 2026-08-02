mod autostart;
mod commands;
mod connection;
mod daemon_host;
mod hotkey;
mod instance;
#[cfg(target_os = "macos")]
mod notch;
mod popup;
mod state;
mod tray;

use std::sync::Arc;
use std::sync::Mutex;

use tauri::Manager;
use tauri_plugin_global_shortcut::ShortcutState;

use state::AppState;

/// `CLIPSE_DATA_DIR` lets a developer point both the app and a standalone
/// daemon at the same temp directory (see the README); production installs fall
/// back to the platform default.
fn resolve_paths() -> clipse_core::Paths {
    let paths = match std::env::var_os("CLIPSE_DATA_DIR") {
        Some(dir) => clipse_core::Paths::with_root(dir),
        None => clipse_core::Paths::platform_default()
            .unwrap_or_else(|_| clipse_core::Paths::with_root(".clipse-dev/app")),
    };
    let _ = paths.create_all();
    paths
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let paths = resolve_paths();
    let default_hotkey = clipse_ipc::Settings::default().hotkey;
    let app_state = Arc::new(AppState::new(paths.ipc_endpoint(), default_hotkey.clone()));
    let connection_state = app_state.clone();

    // Held so the daemon can be told to flush on the way out. `Mutex<Option<_>>`
    // because the exit handler is `Fn`, not `FnOnce`, and must be able to take
    // the handle exactly once.
    let embedded: Arc<Mutex<Option<daemon_host::EmbeddedDaemon>>> = Arc::new(Mutex::new(None));
    let embedded_for_exit = Arc::clone(&embedded);

    tauri::Builder::default()
        // First in the chain, deliberately: this decides whether this process
        // is the app at all, and everything below assumes it is.
        //
        // Without it a second launch could not bind the IPC endpoint, so it
        // became a *client* of the first instance's daemon — two windows over
        // one history, which reads as a second session.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if !instance::should_reveal(&argv) {
                return;
            }
            if let Some(main) = app.get_webview_window("main") {
                let _ = main.unminimize();
                let _ = main.show();
                let _ = main.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        // Clipse is only useful if it is already running when you copy
        // something, so it launches at login by default. `--minimised` is read
        // by `main.rs`: starting at login should put it in the tray, not throw
        // a window at someone who has just signed in.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimised"]),
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    // The hotkey fires on the plugin's own listener thread, and
                    // showing a window — on macOS, reaching into AppKit at all —
                    // is main-thread work.
                    let app = app.clone();
                    let _ = app.clone().run_on_main_thread(move || {
                        let _ = popup::show_near_cursor(&app);
                    });
                })
                .build(),
        )
        .manage(app_state)
        .setup(move |app| {
            let handle = app.handle().clone();

            tray::build(&handle)?;
            hotkey::register_initial(&handle, &default_hotkey);

            // The daemon comes up before anything tries to talk to it.
            let (daemon, ready) = daemon_host::spawn(paths);
            *embedded
                .lock()
                .expect("the exit handler is the only other holder") = daemon;

            // The notch panel is a sidecar process, and a missing one is not an
            // error: it simply is not bundled on every build. The device label
            // is supplied per push, from the daemon's status, so nothing useful
            // is known here yet.
            #[cfg(target_os = "macos")]
            if let Some(panel) =
                notch::Notch::spawn(&handle, Arc::clone(&connection_state), String::new())
            {
                let _ = connection_state.notch.set(panel);
            }

            // The login item is reconciled from `connection.rs`, once the
            // daemon can actually answer what the setting is.
            connection::spawn(handle.clone(), connection_state, ready);

            // Launched by the login item: stay in the tray rather than opening
            // a window over whatever someone is doing as they sign in.
            if std::env::args().any(|arg| arg == "--minimised")
                && let Some(main) = app.get_webview_window("main")
            {
                let _ = main.hide();
            }

            // Closing the window hides it; it does not destroy it.
            //
            // Clipse keeps working with no window open — that is the point of a
            // tray app — and a destroyed window cannot be shown again, so
            // "Open Clipse" in the tray menu silently did nothing for the rest
            // of the session. Quitting is deliberate, from the tray.
            if let Some(main) = app.get_webview_window("main") {
                let closing = main.clone();
                main.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = closing.hide();
                    }
                });
            }

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
            commands::get_payload,
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
            commands::pair_with_code,
            commands::cancel_pairing,
            commands::forget_device,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(move |_handle, event| {
            // Quitting the app has to stop the daemon it started, and give it
            // long enough to write the config and the clock back out.
            if let tauri::RunEvent::Exit = event
                && let Some(mut daemon) = embedded_for_exit
                    .lock()
                    .expect("the setup hook has finished by now")
                    .take()
            {
                daemon.shutdown();
            }
        });
}
