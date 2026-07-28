//! The Clipse daemon as its own process.
//!
//! The desktop app embeds the same code (see `lib.rs`), so this binary exists
//! for the cases a window cannot serve: running headless, keeping sync alive
//! after the last window closes, and the two-daemon end-to-end tests.

use std::path::PathBuf;

use anyhow::Context as _;
use clap::Parser;
use clipse_core::Paths;
use clipsed::{RunOptions, Started};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "clipsed", version, about = "Clipse background daemon")]
struct Args {
    /// Where history, blobs and config live. Defaults to the platform data
    /// directory; the end-to-end test uses it to run two daemons at once.
    #[arg(long, value_name = "DIR")]
    data_dir: Option<PathBuf>,

    /// Log level: error, warn, info, debug, trace.
    #[arg(long, default_value = "info")]
    log: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("CLIPSE_LOG").unwrap_or_else(|_| EnvFilter::new(&args.log)),
        )
        .init();

    let paths = match args.data_dir {
        Some(dir) => Paths::with_root(dir),
        None => Paths::platform_default().context("no platform data directory")?,
    };

    match clipsed::run(RunOptions::new(paths), wait_for_shutdown_signal()).await? {
        Started::Daemon => Ok(()),
        // Failing here is deliberate. The app treats "already running" as a
        // normal outcome and becomes a client, but someone who ran this binary
        // asked for a daemon and did not get one — and a service supervisor
        // reads the exit code to find that out. Two daemons on one data
        // directory would fight over the database.
        Started::AlreadyRunning => {
            anyhow::bail!("clipsed is already running for this data directory")
        }
    }
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
