//! What a second launch means.
//!
//! Clipse could be started twice. The second process could not bind the IPC
//! endpoint, so instead of failing it became a *client* of the first one's
//! daemon — two windows over one history, which reads as a second session of
//! an app that is supposed to have exactly one.
//!
//! The window-revealing half needs a running Tauri app and is verified by hand
//! (see `docs/manual-verification.md`); the decision of *whether* to reveal is
//! pure, and is the part that would otherwise be wrong silently.

/// Whether a second launch should bring the existing window forward.
///
/// A login item that fires twice must not throw a window at someone who is
/// still signing in, so `--minimised` in the *incoming* argv means "stay in the
/// tray" — the same flag `lib.rs` honours on a cold start.
pub fn should_reveal(argv: &[String]) -> bool {
    !argv.iter().any(|arg| arg == "--minimised")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_plain_second_launch_reveals_the_window() {
        assert!(should_reveal(&argv(&["clipse.exe"])));
    }

    #[test]
    fn a_login_item_relaunch_stays_in_the_tray() {
        assert!(!should_reveal(&argv(&["clipse.exe", "--minimised"])));
    }

    #[test]
    fn the_flag_is_recognised_in_any_position() {
        assert!(!should_reveal(&argv(&["clipse.exe", "--minimised", "extra"])));
    }
}
