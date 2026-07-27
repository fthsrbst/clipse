use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("platform clipboard error: {0}")]
    Platform(String),

    #[error("failed to start the clipboard watcher: {0}")]
    WatcherStartup(String),

    #[error("clipboard payload rejected: {0}")]
    InvalidPayload(String),

    #[error(transparent)]
    Core(#[from] clipse_core::Error),
}
