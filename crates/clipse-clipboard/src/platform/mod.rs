//! One module per platform backend, selected entirely at compile time via
//! `#[cfg]` — never at runtime — so a Linux build does not even attempt to
//! link `windows` and vice versa. `start` is the only symbol the rest of the
//! crate calls; everything else here is an implementation detail.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::error::Result;
use crate::own_write_guard::OwnWriteGuard;
use crate::watch::{CaptureEvent, WatchConfig, Watcher};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as imp;

#[cfg(all(unix, not(target_os = "macos")))]
mod linux;
#[cfg(all(unix, not(target_os = "macos")))]
use linux as imp;

pub(crate) fn start(
    config: WatchConfig,
    guard: Arc<OwnWriteGuard>,
    tx: mpsc::Sender<CaptureEvent>,
) -> Result<Watcher> {
    imp::start(config, guard, tx)
}
