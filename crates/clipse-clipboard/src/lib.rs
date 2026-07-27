//! Platform clipboard capture and injection for Clipse.
//!
//! Watches the OS clipboard, turns changes into [`Capture`]s, suppresses
//! anything sensitive before it ever becomes a `Clip`, and writes content
//! back to the clipboard when a clip arrives from another device.
//!
//! Everything that can be tested without an OS clipboard — the suppression
//! pipeline, the sensitive-content detectors, the own-write loop guard — is
//! plain, pure Rust in [`sensitive`] and [`own_write_guard`], independent of
//! which platform backend (if any, on this build) is compiled in.

mod capture;
mod error;
mod own_write_guard;
mod platform;
pub mod sensitive;
mod watch;

pub use capture::{Capture, Clipboard};
pub use error::{Error, Result};
pub use own_write_guard::{DEFAULT_OWN_WRITE_TTL, OwnWriteGuard};
pub use watch::{
    CaptureEvent, DEFAULT_MAX_PAYLOAD_BYTES, SuppressionReason, WatchConfig, WatchMode, Watcher,
    watch,
};
