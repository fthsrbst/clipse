//! The content-addressed blob store and its LRU disk quota.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use clipse_core::ContentHash;
use rusqlite::{Transaction, params};

use crate::error::{Error, Result};
use crate::store::{LOCK_POISONED, Store};

/// Outcome of one [`Store::enforce_blob_quota`] pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaReport {
    pub quota_bytes: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub blobs_evicted: usize,
    pub bytes_evicted: u64,
}

/// `blobs/<ab>/<cd>/<64-hex>` under the given root -- shallow sharding keeps
/// any one directory from accumulating tens of thousands of entries.
fn shard_path(root: &Path, digest: &ContentHash) -> PathBuf {
    let (a, b, full) = digest.shard_path();
    root.join(a).join(b).join(full)
}

impl Store {
    pub fn blob_path(&self, digest: &ContentHash) -> PathBuf {
        shard_path(&self.blobs_root, digest)
    }

    pub fn has_blob(&self, digest: &ContentHash) -> Result<bool> {
        Ok(self.blob_path(digest).is_file())
    }

    /// Writes `bytes` at `digest`'s content-addressed path and records it for
    /// LRU accounting. Idempotent: if the path is already occupied its bytes
    /// cannot legitimately differ (the path *is* the hash of the bytes), so
    /// only the recency bookkeeping is refreshed.
    pub fn put_blob(&self, digest: &ContentHash, bytes: &[u8]) -> Result<()> {
        let path = self.blob_path(digest);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if !path.is_file() {
            // Write to a temp file and rename into place so a crash
            // mid-write can never leave a truncated file at the final path
            // for `has_blob`/`read_blob` to trust.
            let tmp = path.with_extension("tmp");
            fs::write(&tmp, bytes)?;
            fs::rename(&tmp, &path)?;
        }

        let seq = self.next_used_seq();
        let conn = self.conn.lock().expect(LOCK_POISONED);
        conn.execute(
            "INSERT INTO blob_meta (digest, size, used_seq) VALUES (?1, ?2, ?3)
             ON CONFLICT(digest) DO UPDATE SET used_seq = excluded.used_seq",
            params![digest.to_hex(), bytes.len() as i64, seq],
        )?;
        Ok(())
    }

    /// Reads a blob's bytes back and counts the read as a use for LRU
    /// purposes -- a blob a peer keeps re-fetching should be treated as
    /// recently relevant, the same as one just written.
    pub fn read_blob(&self, digest: &ContentHash) -> Result<Vec<u8>> {
        let path = self.blob_path(digest);
        let bytes = fs::read(&path).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                Error::BlobNotFound(*digest)
            } else {
                Error::Io(err)
            }
        })?;

        let seq = self.next_used_seq();
        let conn = self.conn.lock().expect(LOCK_POISONED);
        conn.execute(
            "UPDATE blob_meta SET used_seq = ?1 WHERE digest = ?2",
            params![seq, digest.to_hex()],
        )?;
        Ok(bytes)
    }

    /// Evicts least-recently-used blobs until total blob storage is at or
    /// under the configured quota.
    ///
    /// Never evicts a blob referenced by any live (non-deleted), pinned
    /// clip's payload -- even if that same digest is *also* referenced by an
    /// unpinned one elsewhere. Never touches inline payloads at all: they
    /// have no `blob_meta` row to begin with, since only payloads over
    /// `INLINE_MAX_BYTES` are written to the blob store in the first place.
    /// The `clip`/`payload` rows always survive eviction -- only the on-disk
    /// bytes and the `blob_meta` entry are removed, which is exactly what
    /// makes `Clip::is_complete` observe the loss afterwards.
    pub fn enforce_blob_quota(&self) -> Result<QuotaReport> {
        let conn = self.conn.lock().expect(LOCK_POISONED);

        let bytes_before: i64 =
            conn.query_row("SELECT COALESCE(SUM(size), 0) FROM blob_meta", [], |row| {
                row.get(0)
            })?;
        let mut bytes_used = bytes_before.max(0) as u64;
        let mut blobs_evicted = 0usize;
        let mut bytes_evicted = 0u64;

        if bytes_used > self.blob_quota_bytes {
            let candidates: Vec<(String, i64)> = {
                let mut stmt = conn.prepare(
                    "SELECT digest, size FROM blob_meta
                     WHERE digest NOT IN (
                         SELECT DISTINCT p.digest FROM payload p
                         JOIN clip c ON c.id = p.clip_id
                         WHERE c.pinned = 1 AND c.deleted = 0
                     )
                     ORDER BY used_seq ASC",
                )?;
                stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };

            for (digest_hex, size) in candidates {
                if bytes_used <= self.blob_quota_bytes {
                    break;
                }
                let digest: ContentHash = digest_hex.parse()?;
                let path = self.blob_path(&digest);
                if path.is_file() {
                    fs::remove_file(&path)?;
                }
                conn.execute(
                    "DELETE FROM blob_meta WHERE digest = ?1",
                    params![digest_hex],
                )?;
                bytes_used = bytes_used.saturating_sub(size.max(0) as u64);
                blobs_evicted += 1;
                bytes_evicted += size.max(0) as u64;
            }
        }

        Ok(QuotaReport {
            quota_bytes: self.blob_quota_bytes,
            bytes_before: bytes_before.max(0) as u64,
            bytes_after: bytes_used,
            blobs_evicted,
            bytes_evicted,
        })
    }

    pub(crate) fn next_used_seq(&self) -> i64 {
        // `+ 1` so the very first call (fetch_add returns the pre-increment
        // 0) yields 1, keeping "no blob touched yet" (0, from `open`'s seed
        // query) distinguishable from "touched first".
        (self.used_seq.fetch_add(1, Ordering::SeqCst) + 1) as i64
    }
}

/// Removes `blob_meta` rows (and their on-disk files) that no `payload` row
/// references any more. Called after `purge_tombstones` cascades away
/// payload rows, since that is the only path that can make a digest go from
/// referenced to orphaned.
///
/// Best-effort with respect to the filesystem: the `blob_meta` deletion is
/// part of the caller's transaction, but the file removal is not (SQLite has
/// no concept of a transactional file delete). If the process dies between
/// the two, the file is merely orphaned on disk -- the next
/// `enforce_blob_quota` pass or `purge_tombstones` run will find it has no
/// `blob_meta` row and skip it, and nothing worse than a delayed reclaim
/// happens. Making this fully crash-safe would need a separate
/// write-ahead-of-intent log for filesystem deletes, which is more machinery
/// than a local cache's worth of slack justifies.
pub(crate) fn prune_orphan_blobs(tx: &Transaction<'_>, blobs_root: &Path) -> Result<()> {
    let orphans: Vec<String> = {
        let mut stmt = tx.prepare(
            "SELECT digest FROM blob_meta
             WHERE digest NOT IN (SELECT DISTINCT digest FROM payload)",
        )?;
        stmt.query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };

    for digest_hex in orphans {
        let digest: ContentHash = digest_hex.parse()?;
        let path = shard_path(blobs_root, &digest);
        if path.is_file() {
            fs::remove_file(&path)?;
        }
        tx.execute(
            "DELETE FROM blob_meta WHERE digest = ?1",
            params![digest_hex],
        )?;
    }
    Ok(())
}
