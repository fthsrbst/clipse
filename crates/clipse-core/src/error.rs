use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid content hash: {0}")]
    InvalidHash(String),

    #[error("invalid device id: {0}")]
    InvalidDeviceId(String),

    #[error("clock moved backwards beyond the accepted drift ({drift_ms} ms)")]
    ClockDrift { drift_ms: u64 },

    #[error("unsupported clipboard format: {0}")]
    UnsupportedFormat(String),

    #[error("could not determine a platform data directory")]
    NoDataDirectory,
}
