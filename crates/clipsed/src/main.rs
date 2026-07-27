//! The Clipse daemon.
//!
//! Owns the clipboard, the history and (from F2) the peer sessions. User
//! interfaces are clients over `clipse-ipc`; closing a window must never stop
//! syncing, which is the whole reason this is a separate process.

mod config;
mod daemon;
mod ipc_server;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use clipse_core::Paths;
use clipse_ipc::transport::Listener;
use tracing::info;
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
    paths.create_all().with_context(|| format!("creating {}", paths.root().display()))?;

    let config = config::Config::load_or_create(&paths)?;
    info!(device = %config.device.short(), root = %paths.root().display(), "starting clipsed");

    let listener = Listener::bind(&paths.ipc_endpoint()).await?;
    info!(endpoint = listener.endpoint(), "ipc listening");

    let daemon = Arc::new(daemon::Daemon::new(paths, config));
    let server = ipc_server::IpcServer::new(Arc::clone(&daemon));
    daemon.set_event_sink(server.events());

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let serving = tokio::spawn(server.serve(listener, shutdown_rx));

    wait_for_shutdown_signal().await;
    info!("shutting down");
    let _ = shutdown_tx.send(true);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), serving).await;

    daemon.persist()?;
    Ok(())
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
