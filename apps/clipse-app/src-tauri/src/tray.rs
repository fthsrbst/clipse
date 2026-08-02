//! The tray icon: sync status line, the paired devices and how they are
//! reachable, Pause/Resume, Open Clipse, Quit.
//!
//! # Why the menu is rebuilt rather than patched
//!
//! The status line and the Pause item are fixed rows, so they are updated in
//! place. The device rows are not: their *number* changes when a device is
//! paired or forgotten, and a menu item cannot be inserted into a built
//! `Menu` on every platform Tauri supports. So the whole menu is rebuilt from
//! a small model whenever the device list changes, and handed to the tray with
//! `set_menu`. Rebuilding a seven-item menu costs nothing; a tray that says
//! "No paired devices" next to two paired devices costs the user their trust
//! in the thing.

use std::sync::Mutex;

use clipse_ipc::{Connectivity, DaemonStatus, PeerInfo};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

/// Everything the menu shows, kept so a change to one part can be redrawn
/// without asking the daemon for the others.
#[derive(Default)]
struct TrayModel {
    status: Option<DaemonStatus>,
    devices: Vec<PeerInfo>,
    connected: bool,
}

pub struct TrayState {
    model: Mutex<TrayModel>,
}

const TRAY_ID: &str = "clipse-tray";

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    app.manage(TrayState {
        model: Mutex::new(TrayModel::default()),
    });

    let icon = app.default_window_icon().cloned();
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu_for(app, &TrayModel::default())?)
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
    Ok(())
}

/// Called whenever a fresh `DaemonStatus` arrives, from either the initial
/// fetch after connecting or a pushed `StatusChanged` event.
pub fn on_status(app: &AppHandle, status: &DaemonStatus) {
    update(app, |model| {
        model.status = Some(status.clone());
        model.connected = true;
    });
}

/// Called with the full device list after connecting, and whenever one of them
/// changes.
pub fn on_devices(app: &AppHandle, devices: Vec<PeerInfo>) {
    update(app, |model| model.devices = devices);
}

/// One device changed — it came online, or a session just finished with it.
pub fn on_device_changed(app: &AppHandle, device: PeerInfo) {
    update(app, |model| {
        match model
            .devices
            .iter_mut()
            .find(|known| known.device == device.device)
        {
            Some(known) => *known = device,
            None => model.devices.push(device),
        }
    });
}

/// Called when the connection to the daemon drops.
pub fn on_disconnected(app: &AppHandle) {
    update(app, |model| {
        model.connected = false;
        // The device list is *not* cleared: those devices are still paired, and
        // saying otherwise because our own daemon went away would be a second
        // lie on top of the first.
        for device in &mut model.devices {
            device.connectivity = Connectivity::Offline;
        }
    });
}

fn update(app: &AppHandle, change: impl FnOnce(&mut TrayModel)) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };
    let mut model = state.model.lock().expect("tray model poisoned");
    change(&mut model);

    let Ok(menu) = menu_for(app, &model) else {
        return;
    };
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_menu(Some(menu));
        let _ = tray.set_tooltip(Some(tooltip(&model)));
    }
}

fn menu_for(app: &AppHandle, model: &TrayModel) -> tauri::Result<Menu<Wry>> {
    // Disabled items throughout: these rows are a readout, not controls. A
    // clickable line that does nothing is worse than a greyed-out one.
    let status = MenuItem::with_id(app, "status", status_line(model), false, None::<&str>)?;

    let mut items: Vec<Box<dyn tauri::menu::IsMenuItem<Wry>>> = vec![
        Box::new(status),
        Box::new(PredefinedMenuItem::separator(app)?),
    ];

    if model.devices.is_empty() {
        items.push(Box::new(MenuItem::with_id(
            app,
            "devices-empty",
            "No paired devices",
            false,
            None::<&str>,
        )?));
    } else {
        for device in &model.devices {
            items.push(Box::new(MenuItem::with_id(
                app,
                format!("device-{}", device.device),
                device_line(device),
                false,
                None::<&str>,
            )?));
        }
    }

    let paused = model.status.as_ref().is_some_and(|status| status.paused);
    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    items.push(Box::new(MenuItem::with_id(
        app,
        "toggle-pause",
        if paused {
            "Resume syncing"
        } else {
            "Pause syncing"
        },
        model.connected,
        None::<&str>,
    )?));
    items.push(Box::new(MenuItem::with_id(
        app,
        "open",
        "Open Clipse",
        true,
        None::<&str>,
    )?));
    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    items.push(Box::new(MenuItem::with_id(
        app,
        "quit",
        "Quit",
        true,
        None::<&str>,
    )?));

    let refs: Vec<&dyn tauri::menu::IsMenuItem<Wry>> =
        items.iter().map(|item| item.as_ref()).collect();
    Menu::with_items(app, &refs)
}

fn tooltip(model: &TrayModel) -> String {
    match &model.status {
        Some(_) if model.connected => status_line(model),
        _ => "Clipse".to_string(),
    }
}

/// The first line of the menu, which is the one thing a user reads before
/// deciding whether Clipse is working.
fn status_line(model: &TrayModel) -> String {
    if !model.connected {
        return "Clipse — daemon not running".to_string();
    }
    let Some(status) = &model.status else {
        return "Connecting to Clipse…".to_string();
    };

    let clips = plural(status.clip_count, "clip");
    if status.paused {
        return format!("Clipse — paused ({clips})");
    }

    let online = model
        .devices
        .iter()
        .filter(|device| device.connectivity != Connectivity::Offline)
        .count();
    match (model.devices.len(), online) {
        (0, _) => format!("Clipse — syncing ({clips})"),
        (_, 0) => format!("Clipse — {clips}, no device reachable"),
        (total, online) if online == total => {
            format!(
                "Clipse — syncing with {}, {clips}",
                plural(total as u64, "device")
            )
        }
        (total, online) => format!("Clipse — {online} of {total} devices, {clips}"),
    }
}

fn device_line(device: &PeerInfo) -> String {
    let where_ = match device.connectivity {
        Connectivity::Lan => "on this network".to_string(),
        Connectivity::Tailnet => "over Tailscale".to_string(),
        Connectivity::Offline => match device.last_seen_ms {
            Some(seen) => format!("last synced {}", ago(seen)),
            None => "not reached yet".to_string(),
        },
    };
    format!("{} — {where_}", device.label)
}

/// Coarse on purpose: this is a tray menu, not a log. Minutes are the finest
/// unit worth reading at a glance.
fn ago(then_ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let minutes = now.saturating_sub(then_ms) / 60_000;
    match minutes {
        0 => "just now".to_string(),
        1 => "1 minute ago".to_string(),
        m if m < 60 => format!("{m} minutes ago"),
        m if m < 120 => "an hour ago".to_string(),
        m if m < 60 * 24 => format!("{} hours ago", m / 60),
        _ => "more than a day ago".to_string(),
    }
}

fn plural(count: u64, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        // Unminimise first: a minimised window answers `show()` without ever
        // coming back into view, which looks exactly like the menu item being
        // broken.
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use clipse_core::DeviceId;

    use super::*;

    fn peer(label: &str, connectivity: Connectivity, last_seen_ms: Option<u64>) -> PeerInfo {
        PeerInfo {
            device: DeviceId::generate(),
            label: label.to_string(),
            platform: "macos".into(),
            connectivity,
            last_seen_ms,
        }
    }

    fn status(clip_count: u64, paused: bool) -> DaemonStatus {
        DaemonStatus {
            device: DeviceId::generate(),
            device_label: "here".into(),
            daemon_version: "test".into(),
            paused,
            capture_mode: clipse_ipc::CaptureMode::Automatic,
            clip_count,
            blob_bytes: 0,
            blob_quota_bytes: 0,
            peers_online: 0,
            peers_total: 0,
            secrets_refused: 0,
        }
    }

    #[test]
    fn the_status_line_says_what_is_actually_happening() {
        let mut model = TrayModel::default();
        assert_eq!(status_line(&model), "Clipse — daemon not running");

        model.connected = true;
        assert_eq!(status_line(&model), "Connecting to Clipse…");

        model.status = Some(status(158, false));
        assert_eq!(status_line(&model), "Clipse — syncing (158 clips)");

        model.devices = vec![peer("PC", Connectivity::Lan, Some(0))];
        assert_eq!(
            status_line(&model),
            "Clipse — syncing with 1 device, 158 clips"
        );

        model.devices.push(peer("Pi", Connectivity::Offline, None));
        assert_eq!(status_line(&model), "Clipse — 1 of 2 devices, 158 clips");

        model.devices = vec![peer("PC", Connectivity::Offline, None)];
        assert_eq!(
            status_line(&model),
            "Clipse — 158 clips, no device reachable",
            "a tray that claims to be syncing with nothing is the bug this replaces"
        );

        model.status = Some(status(1, true));
        assert_eq!(status_line(&model), "Clipse — paused (1 clip)");
    }

    #[test]
    fn a_device_row_says_where_it_is() {
        assert_eq!(
            device_line(&peer("Fatih's PC", Connectivity::Lan, Some(0))),
            "Fatih's PC — on this network"
        );
        assert_eq!(
            device_line(&peer("Pi", Connectivity::Tailnet, Some(0))),
            "Pi — over Tailscale"
        );
        assert_eq!(
            device_line(&peer("Pi", Connectivity::Offline, None)),
            "Pi — not reached yet"
        );
    }

    #[test]
    fn a_dropped_daemon_does_not_forget_the_paired_devices() {
        // The devices are still paired; only their reachability is unknown.
        let mut model = TrayModel {
            connected: true,
            status: Some(status(3, false)),
            devices: vec![peer("PC", Connectivity::Lan, Some(0))],
        };
        model.connected = false;
        for device in &mut model.devices {
            device.connectivity = Connectivity::Offline;
        }
        assert_eq!(model.devices.len(), 1);
        assert_eq!(status_line(&model), "Clipse — daemon not running");
    }
}
