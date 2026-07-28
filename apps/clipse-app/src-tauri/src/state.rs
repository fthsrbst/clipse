//! Shared application state.
//!
//! One [`AppState`] is managed by Tauri and handed to every command and to the
//! background connection task. The command connection lives behind a
//! `tokio::sync::Mutex` because [`clipse_ipc::Client::call`] is not safe to
//! drive from two tasks at once: it writes a request, then reads frames until
//! it sees the matching id, so a second concurrent caller could steal a
//! response that was not addressed to it.

use std::sync::Mutex as SyncMutex;

use clipse_ipc::Client;
use tokio::sync::Mutex as AsyncMutex;

pub struct AppState {
    /// Local IPC endpoint (named pipe path on Windows, socket path elsewhere).
    pub endpoint: String,
    /// `None` whenever the daemon is unreachable; the reconnect loop in
    /// `connection.rs` owns writing to this.
    pub client: AsyncMutex<Option<Client>>,
    /// The hotkey currently registered with the OS, so settings updates know
    /// what to unregister before registering the new one.
    pub current_hotkey: SyncMutex<String>,
    /// The notch panel, once it has been started.
    ///
    /// A `OnceLock` because it is set exactly once during setup and read from
    /// the connection task afterwards: there is nothing to lock for, and the
    /// sidecar is missing on a machine where it was never bundled, which is a
    /// normal state rather than an error.
    #[cfg(target_os = "macos")]
    pub notch: std::sync::OnceLock<std::sync::Arc<crate::notch::Notch>>,
}

impl AppState {
    pub fn new(endpoint: String, initial_hotkey: String) -> Self {
        Self {
            endpoint,
            client: AsyncMutex::new(None),
            current_hotkey: SyncMutex::new(initial_hotkey),
            #[cfg(target_os = "macos")]
            notch: std::sync::OnceLock::new(),
        }
    }
}
