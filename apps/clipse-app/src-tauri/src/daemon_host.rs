//! Runs the daemon inside the app process, so Clipse is one executable.
//!
//! Clipse is still a daemon with a window in front of it: this module starts
//! `clipsed` on a background task and then forgets about it. The UI reaches it
//! the same way it always has, over `clipse-ipc` — nothing here hands the
//! window a `Store` or a socket. What changes is only how many files someone
//! has to install, which was never supposed to be their problem.
//!
//! Whoever binds the IPC endpoint first is the daemon. If a standalone
//! `clipsed` already owns the data directory, [`spawn`] steps aside and the app
//! is simply its client.

use std::sync::mpsc;
use std::time::Duration;

use clipse_core::Paths;
use clipsed::{RunOptions, Started};
use tokio::sync::oneshot;
use tracing::{info, warn};

/// How long to wait for the daemon to flush its state when the app quits.
///
/// The daemon writes the config and the HLC on the way out. Quitting is not a
/// good enough reason to lose them, and it is not a good enough reason to hang
/// either, so it gets a bounded moment.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(4);

pub struct EmbeddedDaemon {
    shutdown: Option<oneshot::Sender<()>>,
    stopped: mpsc::Receiver<()>,
}

impl EmbeddedDaemon {
    /// Ask the daemon to stop, and wait briefly for it to finish writing.
    pub fn shutdown(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        match self.stopped.recv_timeout(SHUTDOWN_GRACE) {
            Ok(()) => info!("embedded daemon stopped"),
            Err(_) => warn!("embedded daemon did not stop in time; state may be a moment stale"),
        }
    }
}

/// Start the daemon in this process.
///
/// Returns the handle, and a receiver that fires once the daemon is answering
/// requests. `None` for the handle means another process is the daemon.
pub fn spawn(paths: Paths) -> (Option<EmbeddedDaemon>, oneshot::Receiver<()>) {
    let (ready_tx, ready_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (stopped_tx, stopped_rx) = mpsc::channel();

    tauri::async_runtime::spawn(async move {
        let options = RunOptions::new(paths).with_ready(ready_tx);
        let outcome = clipsed::run(options, async move {
            // A dropped sender means the app is going away without a clean
            // signal; treat it as one rather than waiting forever.
            let _ = shutdown_rx.await;
        })
        .await;

        match outcome {
            Ok(Started::Daemon) => {}
            Ok(Started::AlreadyRunning) => {
                info!("using the clipsed that already owns this data directory")
            }
            // The window is still worth showing: it will report the daemon as
            // unreachable, which is the truth and is more useful than a
            // process that dies on launch with a message nobody sees.
            Err(e) => warn!(error = %e, "the embedded daemon stopped early"),
        }
        let _ = stopped_tx.send(());
    });

    let handle = EmbeddedDaemon {
        shutdown: Some(shutdown_tx),
        stopped: stopped_rx,
    };
    (Some(handle), ready_rx)
}
