//! Positions and shows the hotkey popup near the cursor, clamped to the
//! monitor it landed on so it never spawns half off-screen on a multi-monitor
//! setup.
//!
//! # macOS and full-screen apps
//!
//! A full-screen app on macOS is its own Space, and an ordinary window belongs
//! to the Space it was created in — so the popup simply did not appear when
//! the hotkey was pressed over a full-screen window. The window was shown, it
//! took focus, and the user saw nothing.
//!
//! Two AppKit properties fix that, and both are needed: a collection behaviour
//! of `canJoinAllSpaces | fullScreenAuxiliary` (join whatever Space is in
//! front, including a full-screen one), and a window level above the
//! full-screen app's own. They are applied every time the popup is shown
//! rather than once at startup, because a window recreated by the OS — or by a
//! display change — comes back with the defaults.

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

    // Before `show`: a window that is about to appear over a full-screen app
    // has to already be allowed into that Space, or the first frame lands in
    // the wrong one.
    #[cfg(target_os = "macos")]
    join_every_space(&popup);

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

/// Let the popup appear over a full-screen app instead of behind it.
///
/// `set_visible_on_all_workspaces` alone is not enough: tao sets
/// `canJoinAllSpaces` and nothing else, and a window without
/// `fullScreenAuxiliary` is still refused entry to a full-screen Space. So the
/// collection behaviour is set directly, and the level is raised above the
/// full-screen app's window level — `NSPopUpMenuWindowLevel`, the level menus
/// use, which is exactly the role this window plays.
#[cfg(target_os = "macos")]
fn join_every_space(popup: &tauri::WebviewWindow) {
    use objc2_app_kit::{NSWindow, NSWindowCollectionBehavior};

    // Tao's own flag, first: it also flips the internal state tao consults
    // when it recreates or re-shows the window, so setting only the AppKit
    // property would be undone the next time tao touched the window.
    let _ = popup.set_visible_on_all_workspaces(true);
    let _ = popup.set_always_on_top(true);

    let Ok(handle) = popup.ns_window() else {
        return;
    };
    if handle.is_null() {
        return;
    }

    // SAFETY: `ns_window` hands back this window's `NSWindow`, which tao owns
    // for as long as the window exists — and it exists, because the handle was
    // obtained from it a line ago. Both calls are ordinary AppKit setters made
    // on the thread the window lives on (see the caller: the hotkey handler
    // hops to the main thread before showing the popup).
    unsafe {
        let window: &NSWindow = &*(handle as *const NSWindow);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                // Keeps it out of Cmd-Tab and Exposé: this is a menu, not a
                // window someone should be able to cycle back to.
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        // 101. Above a full-screen app's own window, below the screen saver.
        window.setLevel(objc2_app_kit::NSPopUpMenuWindowLevel);
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
