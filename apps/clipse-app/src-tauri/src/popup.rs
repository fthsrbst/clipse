//! Positions and shows the hotkey popup near the cursor, clamped to the
//! monitor it landed on so it never spawns half off-screen on a multi-monitor
//! setup.

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition};

pub const LABEL: &str = "popup";

/// Told to the popup's webview every time it comes up.
///
/// The window is hidden between uses rather than destroyed, so its JS never
/// mounts a second time and has no other way to know it is on screen again —
/// which is when it needs to replay its entrance.
const SHOWN_EVENT: &str = "popup:shown";

pub fn show_near_cursor(app: &AppHandle) -> tauri::Result<()> {
    let Some(popup) = app.get_webview_window(LABEL) else {
        return Ok(());
    };

    if let Ok((x, y)) = target_position(app, &popup) {
        let _ = popup.set_position(PhysicalPosition::new(x, y));
    }

    popup.show()?;
    popup.set_focus()?;
    // After show, not before: the animation should start on a frame the window
    // is actually visible for.
    let _ = popup.emit(SHOWN_EVENT, ());
    Ok(())
}

pub fn hide(app: &AppHandle) {
    if let Some(popup) = app.get_webview_window(LABEL) {
        let _ = popup.hide();
    }
}

/// Cursor position clamped so the popup's full footprint stays on the
/// monitor the cursor is currently over.
fn target_position(app: &AppHandle, popup: &tauri::WebviewWindow) -> tauri::Result<(f64, f64)> {
    let cursor = app.cursor_position()?;
    let outer = popup.outer_size()?;
    let (width, height) = (outer.width as f64, outer.height as f64);

    let monitor = app
        .monitor_from_point(cursor.x, cursor.y)?
        .or(popup.current_monitor()?);

    let Some(monitor) = monitor else {
        return Ok((cursor.x, cursor.y));
    };

    let pos = monitor.position();
    let size = monitor.size();
    let (min_x, min_y) = (pos.x as f64, pos.y as f64);
    // `.max(min_*)` guards a monitor smaller than the popup itself.
    let max_x = (min_x + size.width as f64 - width).max(min_x);
    let max_y = (min_y + size.height as f64 - height).max(min_y);

    Ok((cursor.x.clamp(min_x, max_x), cursor.y.clamp(min_y, max_y)))
}
