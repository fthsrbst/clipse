//! Persistent clip history and blob store for Clipse.
//!
//! Text history is unlimited and never evicted; only large payloads spilled
//! to the on-disk blob store are subject to a quota. See `Store`'s doc
//! comment for how the synchronous SQLite access here is meant to be called
//! from the async daemon.

mod blob;
#[cfg(feature = "encryption")]
mod encryption;
mod error;
mod fts;
mod row;
mod schema;
mod store;

pub use blob::QuotaReport;
#[cfg(feature = "encryption")]
pub use encryption::EncryptionKey;
pub use error::{Error, Result};
pub use store::{DEFAULT_BLOB_QUOTA_BYTES, HistoryQuery, InsertOutcome, Store, StoreOptions};
