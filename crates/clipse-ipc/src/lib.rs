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

/// Bumped when a client and a daemon can no longer understand each other.
/// Independent of `clipse_core::PROTOCOL_VERSION`, which governs the *network*
/// format between two daemons.
pub const IPC_VERSION: u16 = 1;
