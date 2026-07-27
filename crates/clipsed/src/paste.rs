//! Synthetic paste.
//!
//! `Request::Paste` puts a clip on the clipboard and then presses the paste
//! shortcut for the user, so that choosing an entry in the hotkey popup lands
//! the text in whatever they were typing in.
//!
//! This lives in the daemon rather than in the UI because every front end —
//! the Tauri popup, the macOS notch panel, a future CLI — needs exactly the
//! same behaviour, and the caller has already given up focus by the time it
//! asks for this.

use std::time::Duration;

use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use tracing::debug;

/// Gap between putting the content on the clipboard and pressing the keys.
///
/// The caller hides its popup immediately before asking for a paste, and the
/// window it was covering has to actually get focus back before the keystroke
/// arrives — otherwise the paste goes to a window that is on its way out.
const FOCUS_SETTLE: Duration = Duration::from_millis(80);

#[derive(Debug, thiserror::Error)]
pub enum PasteError {
    #[error("could not reach the input system: {0}")]
    Unavailable(String),

    #[error("could not synthesise the paste keystroke: {0}")]
    Keystroke(String),
}

/// The platform's paste modifier. macOS uses Command; everywhere else Control.
fn paste_modifier() -> Key {
    #[cfg(target_os = "macos")]
    {
        Key::Meta
    }
    #[cfg(not(target_os = "macos"))]
    {
        Key::Control
    }
}

/// Press the paste shortcut. Blocking — call it from `spawn_blocking`.
pub fn press_paste_shortcut() -> Result<(), PasteError> {
    std::thread::sleep(FOCUS_SETTLE);

    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| PasteError::Unavailable(e.to_string()))?;

    let modifier = paste_modifier();
    enigo
        .key(modifier, Direction::Press)
        .map_err(|e| PasteError::Keystroke(e.to_string()))?;
    let result = enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| PasteError::Keystroke(e.to_string()));
    // Released even if the 'v' failed: leaving a modifier stuck down would
    // make the user's keyboard behave bizarrely until they pressed it again.
    let release = enigo
        .key(modifier, Direction::Release)
        .map_err(|e| PasteError::Keystroke(e.to_string()));

    result.and(release)?;
    debug!("synthetic paste sent");
    Ok(())
}
