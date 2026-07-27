//! Key handling for the `encryption` feature (SQLCipher via `PRAGMA key`).
//!
//! Entirely compiled out when the feature is off, so the non-encrypted path
//! never depends on this module existing — see `docs/decisions.md` for why
//! the feature defaults off.

use rusqlite::Connection;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{Error, Result};

/// A 32-byte database encryption key. Zeroized on drop so a key does not
/// linger in freed memory after the `Store` (or a `StoreOptions`) is dropped.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct EncryptionKey([u8; 32]);

impl EncryptionKey {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let array: [u8; 32] = bytes.try_into().map_err(|_| Error::InvalidKeyLength)?;
        Ok(Self(array))
    }
}

/// Applies the key immediately after opening the connection and before any
/// other statement runs, as SQLCipher requires: every subsequent read/write
/// on this handle is transparently encrypted/decrypted with it. `PRAGMA key`
/// takes a hex blob literal (`x'...'`) rather than a bound parameter — SQLite
/// pragmas do not accept bind parameters — so the key is hex-encoded inline.
/// That is safe here because the value is our own fixed-width byte array, not
/// attacker-controlled input.
pub(crate) fn apply_key(conn: &Connection, key: &EncryptionKey) -> Result<()> {
    let hex_key = hex::encode(key.0);
    conn.execute_batch(&format!("PRAGMA key = \"x'{hex_key}'\";"))?;
    Ok(())
}
