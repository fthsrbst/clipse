//! Shared domain types for Clipse.
//!
//! Every other crate in the workspace depends on this one and nothing in here
//! depends on them. Keep it free of I/O, async and platform code so the types
//! stay usable from the daemon, the Tauri app and the test harnesses alike.

pub mod clip;
pub mod error;
pub mod hash;
pub mod hlc;
pub mod id;
pub mod paths;

pub use clip::{Clip, ClipFormat, ClipKind, ClipSource, Payload, PayloadBody, INLINE_MAX_BYTES};
pub use error::{Error, Result};
pub use hash::ContentHash;
pub use hlc::{Hlc, HlcClock, MAX_CLOCK_DRIFT_MS};
pub use id::{ClipId, DeviceId};
pub use paths::Paths;

/// Bumped whenever the wire format between two daemons changes incompatibly.
pub const PROTOCOL_VERSION: u16 = 1;
