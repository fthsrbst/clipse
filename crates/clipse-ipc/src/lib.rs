//! The daemon ⇄ user-interface protocol.
//!
//! `clipsed` owns the clipboard, the history and the sync sessions. Every user
//! interface — the Tauri app, the hotkey popup, the macOS notch panel, a future
//! CLI — is a client over this protocol. Nothing but the daemon touches the
//! store or the network.
//!
//! Frames are length-prefixed MessagePack over a unix socket (a named pipe on
//! Windows). The endpoint is derived from the data directory, so a second
//! daemon started against a different `--data-dir` cannot collide with the
//! first — which is what makes the two-daemon end-to-end test possible.

pub mod client;
pub mod codec;
pub mod protocol;
pub mod transport;

pub use client::Client;
pub use codec::{FrameError, MAX_FRAME_BYTES, read_frame, write_frame};
pub use protocol::{
    CaptureMode, Connectivity, DaemonStatus, ErrorCode, Event, Frame, FrameBody, HistoryQuery,
    IpcError, PeerInfo, Request, Response, Settings,
};

/// The largest payload the daemon will hand to a UI for preview.
///
/// A ceiling on *previewing*, not on storing or syncing: a 400MB file copy is
/// still a perfectly good clip, it just is not something to pull into a
/// webview. Sized to clear a 4K screenshot with room to spare while leaving
/// 8MB of headroom under [`MAX_FRAME_BYTES`] — the response is `serde_bytes`,
/// so it encodes 1:1 and this number means what it says.
pub const MAX_PAYLOAD_BYTES: u64 = 24 * 1024 * 1024;

/// Bumped when a client and a daemon can no longer understand each other.
/// Independent of `clipse_core::PROTOCOL_VERSION`, which governs the *network*
/// format between two daemons.
///
/// 2: `DaemonStatus::secrets_refused` and `Request::GetPayload`. The `Hello`
/// handshake refuses a mismatch outright (see `client.rs`), so an addition
/// guarded by a bump needs no optional-field fallback and no degraded path —
/// both sides are guaranteed to agree.
pub const IPC_VERSION: u16 = 2;
