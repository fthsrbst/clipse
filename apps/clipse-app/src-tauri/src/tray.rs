//! The tray icon: sync status line, an (empty, until F2) paired-device list,
//! Pause/Resume, Open Clipse, Quit.

use clipse_ipc::DaemonStatus;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

/// Handles kept around so status pushes and pause toggles can update the menu
/// in place instead of rebuilding it.
pub struct TrayHandles {
    status_item: MenuItem<Wry>,
    pause_item: MenuItem<Wry>,
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let status_item =
        MenuItem::with_id(app, "status", "Connecting to Clipse…", false, None::<&str>)?;
    let devices_placeholder = MenuItem::with_id(
        app,
        "devices-empty",
        "No paired devices",
        false,
        None::<&str>,
    )?;
    let pause_item = MenuItem::with_id(app, "toggle-pause", "Pause syncing", true, None::<&str>)?;
    let open_item = MenuItem::with_id(app, "open", "Open Clipse", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &status_item,
            &PredefinedMenuItem::separator(app)?,
            &devices_placeholder,
            &PredefinedMenuItem::separator(app)?,
            &pause_item,
            &open_item,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;

    let icon = app.default_window_icon().cloned();
    let mut builder = TrayIconBuilder::with_id("clipse-tray")
        .menu(&menu)
        .tooltip("Clipse")
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle-pause" => crate::commands::toggle_pause_from_tray(app.clone()),
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = icon {
        builder = builder.icon(icon);
    }
    builder.build(app)?;

    app.manage(TrayHandles {
        status_item,
        pause_item,
    });
    Ok(())
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Called whenever a fresh `DaemonStatus` arrives, from either the initial
/// fetch after connecting or a pushed `StatusChanged` event.
pub fn on_status(app: &AppHandle, status: &DaemonStatus) {
    let Some(handles) = app.try_state::<TrayHandles>() else {
        return;
    };

    let line = if status.paused {
        format!("Clipse — paused ({} clips)", status.clip_count)
    } else {
        format!("Clipse — syncing ({} clips)", status.clip_count)
    };
    let _ = handles.status_item.set_text(line);

    let pause_label = if status.paused {
        "Resume syncing"
    } else {
        "Pause syncing"
    };
    let _ = handles.pause_item.set_text(pause_label);
}

/// Called when the connection to the daemon drops.
pub fn on_disconnected(app: &AppHandle) {
    let Some(handles) = app.try_state::<TrayHandles>() else {
        return;
    };
    let _ = handles.status_item.set_text("Clipse — daemon not running");
}
