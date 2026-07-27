use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Core(#[from] clipse_core::Error),

    /// The on-disk schema is newer than this build understands. Refusing to
    /// open is the only safe option — running migrations backwards, or
    /// pretending the unknown columns do not exist, both risk corrupting data
    /// a newer daemon still needs.
    #[error(
        "database schema version {found} is newer than the {supported} this build supports; \
         upgrade clipse before opening this database"
    )]
    SchemaTooNew { found: i64, supported: i64 },

    #[error("clip {0} not found")]
    NotFound(clipse_core::ClipId),

    #[error("blob {0} not found in the blob store")]
    BlobNotFound(clipse_core::ContentHash),

    #[cfg(feature = "encryption")]
    #[error("encryption key must be exactly 32 bytes")]
    InvalidKeyLength,
}
