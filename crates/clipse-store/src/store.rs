use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use clipse_core::{Clip, ClipId, ClipKind, ContentHash, Hlc, Paths};
use rusqlite::{Connection, OptionalExtension, named_params, params};

use crate::error::{Error, Result};
use crate::fts;
use crate::row::{RawClip, RawPayload};
use crate::schema;

/// Column list shared by every query that loads a full clip row. Must stay in
/// the same order as `row::RawClip::from_row`.
const CLIP_COLUMNS: &str = "id, hash, kind, preview, source_device, source_device_label, \
     source_app, hlc_wall_ms, hlc_counter, hlc_device, created_at_ms, pinned, deleted";

pub(crate) const LOCK_POISONED: &str = "clipse-store connection mutex poisoned by an earlier panic";

/// The persistent clip history plus the on-disk blob store.
///
/// # Sync store, async caller
///
/// Every method here is synchronous. SQLite's own API is inherently
/// blocking, and there is no async SQLite driver worth the additional FFI
/// surface for what is ultimately a single-writer local database. Rather
/// than fake an async API over a blocking implementation, the sync/async
/// boundary is pushed to the caller: `clipsed` (the async daemon) is
/// expected to call into `Store` via `tokio::task::spawn_blocking`, exactly
/// as it would for any other blocking library. This crate does not depend on
/// tokio at all.
///
/// # One connection behind a mutex, not a pool
///
/// All access goes through a single `rusqlite::Connection` guarded by a
/// `std::sync::Mutex`. A connection pool would only help concurrent
/// *readers*, and this store's actual workload -- a handful of clipboard
/// events and history queries per second, driven by one desktop daemon -- has
/// no read concurrency worth pooling for; a pool would add sizing and
/// checked-out-connection lifetime concerns for no measurable benefit here.
/// WAL mode is still enabled (required by the spec, and the right default
/// regardless): it is what would let a second, read-only connection --
/// a future Tauri-side reader, a backup tool -- open the same file without
/// blocking this daemon's writes. It buys no *intra-process* concurrency
/// today, though, since every call already serializes on the Mutex before it
/// ever reaches SQLite.
pub struct Store {
    pub(crate) conn: Mutex<Connection>,
    pub(crate) blobs_root: PathBuf,
    pub(crate) blob_quota_bytes: u64,
    /// Monotonic counter behind blob LRU ordering. Deliberately not a wall
    /// clock: two evictable blobs written in the same millisecond (routine
    /// in a fast test, and not impossible in production) must still have a
    /// well-defined relative order.
    pub(crate) used_seq: AtomicU64,
}

#[derive(Clone)]
pub struct StoreOptions {
    /// Total bytes the blob store may occupy before `enforce_blob_quota`
    /// starts evicting the least recently used blobs. Text history itself is
    /// never affected -- it lives in the `clip`/`payload` tables, not here.
    pub blob_quota_bytes: u64,
    #[cfg(feature = "encryption")]
    pub encryption_key: Option<crate::encryption::EncryptionKey>,
}

/// 2 GiB, matching the product default described in the crate's task brief.
pub const DEFAULT_BLOB_QUOTA_BYTES: u64 = 2 * 1024 * 1024 * 1024;

impl Default for StoreOptions {
    fn default() -> Self {
        Self {
            blob_quota_bytes: DEFAULT_BLOB_QUOTA_BYTES,
            #[cfg(feature = "encryption")]
            encryption_key: None,
        }
    }
}

impl StoreOptions {
    /// Set the quota without naming the other fields.
    ///
    /// `StoreOptions { blob_quota_bytes, ..Default::default() }` does not
    /// compile the same way with and without the `encryption` feature — with
    /// the feature off there is only one field, and clippy rejects the
    /// `..Default::default()` as needless. Callers use this instead so the
    /// same source builds under both feature sets.
    #[allow(
        clippy::needless_update,
        reason = "the rest of the struct is feature-gated"
    )]
    pub fn with_quota(blob_quota_bytes: u64) -> Self {
        Self {
            blob_quota_bytes,
            ..Default::default()
        }
    }
}

/// Filters shared by [`Store::recent`] and [`Store::search`].
#[derive(Clone, Debug)]
pub struct HistoryQuery {
    pub limit: usize,
    pub offset: usize,
    pub kind: Option<ClipKind>,
    pub pinned_only: bool,
}

/// A derived `Default` would give `limit: 0`, and `LIMIT 0` in SQL returns no
/// rows at all -- a silent footgun for anyone writing
/// `HistoryQuery { kind: Some(..), ..Default::default() }` expecting "no
/// limit applied". A triple-digit default page size is a much safer trap to
/// fall into.
impl Default for HistoryQuery {
    fn default() -> Self {
        Self {
            limit: 100,
            offset: 0,
            kind: None,
            pinned_only: false,
        }
    }
}

/// What happened to the row `Store::insert` was asked to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertOutcome {
    /// A brand-new row was created.
    Inserted(ClipId),
    /// The content already existed (same `ContentHash`); that row's recency
    /// was bumped so it sorts to the top of `recent`, but its id, pinned
    /// state and payloads are untouched.
    Deduplicated(ClipId),
    /// `clip.hash_matches()` was false. Accepting this would let a buggy or
    /// malicious peer claim someone else's content identity, so nothing was
    /// written.
    Rejected,
}

impl Store {
    pub fn open(paths: &Paths, opts: StoreOptions) -> Result<Self> {
        paths.create_all()?;
        let conn = Connection::open(paths.database())?;

        // SQLCipher requires the key before any other statement touches the
        // file -- including the pragmas and schema check right below.
        #[cfg(feature = "encryption")]
        if let Some(key) = &opts.encryption_key {
            crate::encryption::apply_key(&conn, key)?;
        }

        conn.pragma_update(None, "journal_mode", "WAL")?;
        // Required for the `payload` table's `ON DELETE CASCADE` (used by
        // `purge_tombstones`) to actually fire -- SQLite ignores foreign key
        // actions unless this is set, and it is per-connection, not sticky
        // on the file.
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // A second connection (a future reader, or a test) hitting a brief
        // writer lock should wait, not fail immediately.
        conn.busy_timeout(Duration::from_secs(5))?;

        schema::open_and_migrate(&conn)?;

        let used_seq_seed: i64 = conn.query_row(
            "SELECT COALESCE(MAX(used_seq), 0) FROM blob_meta",
            [],
            |row| row.get(0),
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
            blobs_root: paths.blobs(),
            blob_quota_bytes: opts.blob_quota_bytes,
            used_seq: AtomicU64::new(used_seq_seed.max(0) as u64),
        })
    }

    pub fn insert(&self, clip: &Clip) -> Result<InsertOutcome> {
        if !clip.hash_matches() {
            return Ok(InsertOutcome::Rejected);
        }

        let mut conn = self.conn.lock().expect(LOCK_POISONED);
        let tx = conn.transaction()?;
        let hash_hex = clip.hash.to_hex();

        // Dedup only ever matches a *live* row: a hash that belonged to a
        // clip which was since deleted gets a fresh id on re-copy rather
        // than reviving the tombstone, which would otherwise resurrect a
        // row a peer may already believe is gone.
        let existing_id: Option<String> = tx
            .query_row(
                "SELECT id FROM clip WHERE hash = ?1 AND deleted = 0 LIMIT 1",
                params![hash_hex],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(existing_id) = existing_id {
            tx.execute(
                "UPDATE clip SET
                    hlc_wall_ms = ?1, hlc_counter = ?2, hlc_device = ?3,
                    created_at_ms = ?4,
                    source_device = ?5, source_device_label = ?6, source_app = ?7
                 WHERE id = ?8",
                params![
                    clip.hlc.wall_ms as i64,
                    clip.hlc.counter as i64,
                    clip.hlc.device.to_string(),
                    clip.created_at_ms as i64,
                    clip.source.device.to_string(),
                    clip.source.device_label,
                    clip.source.app,
                    existing_id,
                ],
            )?;
            tx.commit()?;
            return Ok(InsertOutcome::Deduplicated(existing_id.parse()?));
        }

        let id_str = clip.id.to_string();
        tx.execute(
            "INSERT INTO clip (id, hash, kind, preview, source_device, source_device_label,
                                source_app, hlc_wall_ms, hlc_counter, hlc_device, created_at_ms,
                                pinned, deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id_str,
                hash_hex,
                clip.kind.as_str(),
                clip.preview,
                clip.source.device.to_string(),
                clip.source.device_label,
                clip.source.app,
                clip.hlc.wall_ms as i64,
                clip.hlc.counter as i64,
                clip.hlc.device.to_string(),
                clip.created_at_ms as i64,
                clip.pinned as i64,
                clip.deleted as i64,
            ],
        )?;

        for payload in &clip.payloads {
            tx.execute(
                "INSERT INTO payload (clip_id, format_label, digest, size, inline_bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id_str,
                    payload.format.label(),
                    payload.digest.to_hex(),
                    payload.size as i64,
                    payload.inline_bytes(),
                ],
            )?;
        }

        tx.execute(
            "INSERT INTO clip_fts (clip_id, body) VALUES (?1, ?2)",
            params![id_str, fts::body_text(clip)],
        )?;

        tx.commit()?;
        Ok(InsertOutcome::Inserted(clip.id))
    }

    pub fn get(&self, id: ClipId) -> Result<Option<Clip>> {
        let conn = self.conn.lock().expect(LOCK_POISONED);
        load_clip(&conn, &id.to_string())
    }

    /// Looks up the live (non-tombstoned) clip with this content hash, if
    /// any. Used publicly for dedup checks by callers that want to decide
    /// before calling `insert`; `insert` itself re-does this check inside
    /// its own transaction rather than trusting a caller's prior call.
    pub fn by_hash(&self, hash: &ContentHash) -> Result<Option<Clip>> {
        let conn = self.conn.lock().expect(LOCK_POISONED);
        let id: Option<String> = conn
            .query_row(
                "SELECT id FROM clip WHERE hash = ?1 AND deleted = 0 LIMIT 1",
                params![hash.to_hex()],
                |row| row.get(0),
            )
            .optional()?;
        match id {
            Some(id) => load_clip(&conn, &id),
            None => Ok(None),
        }
    }

    pub fn recent(&self, query: HistoryQuery) -> Result<Vec<Clip>> {
        let conn = self.conn.lock().expect(LOCK_POISONED);
        let sql = format!(
            "SELECT {CLIP_COLUMNS} FROM clip
             WHERE deleted = 0
               AND (:kind IS NULL OR kind = :kind)
               AND (:pinned_only = 0 OR pinned = 1)
             ORDER BY hlc_wall_ms DESC, hlc_counter DESC, hlc_device DESC
             LIMIT :limit OFFSET :offset"
        );
        let mut stmt = conn.prepare(&sql)?;
        let kind_str = query.kind.map(|k| k.as_str());
        let raws: Vec<RawClip> = stmt
            .query_map(
                named_params! {
                    ":kind": kind_str,
                    ":pinned_only": query.pinned_only as i64,
                    ":limit": query.limit as i64,
                    ":offset": query.offset as i64,
                },
                RawClip::from_row,
            )?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        rows_to_clips(&conn, raws)
    }

    /// Full-text search over previews and `text/plain` payloads (see
    /// `fts::body_text`). `text` is escaped into literal FTS5 tokens (see
    /// `fts::escape_query`), so query-syntax characters in it are always
    /// treated as content to search for, never as operators. An input with
    /// no non-whitespace content matches nothing rather than erroring.
    pub fn search(&self, text: &str, query: HistoryQuery) -> Result<Vec<Clip>> {
        let Some(match_expr) = fts::escape_query(text) else {
            return Ok(Vec::new());
        };

        let conn = self.conn.lock().expect(LOCK_POISONED);
        let sql = format!(
            "SELECT {CLIP_COLUMNS} FROM clip
             WHERE deleted = 0
               AND (:kind IS NULL OR kind = :kind)
               AND (:pinned_only = 0 OR pinned = 1)
               AND id IN (SELECT clip_id FROM clip_fts WHERE clip_fts MATCH :query)
             ORDER BY hlc_wall_ms DESC, hlc_counter DESC, hlc_device DESC
             LIMIT :limit OFFSET :offset"
        );
        let mut stmt = conn.prepare(&sql)?;
        let kind_str = query.kind.map(|k| k.as_str());
        let raws: Vec<RawClip> = stmt
            .query_map(
                named_params! {
                    ":kind": kind_str,
                    ":pinned_only": query.pinned_only as i64,
                    ":query": match_expr,
                    ":limit": query.limit as i64,
                    ":offset": query.offset as i64,
                },
                RawClip::from_row,
            )?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        rows_to_clips(&conn, raws)
    }

    pub fn set_pinned(&self, id: ClipId, pinned: bool) -> Result<()> {
        let conn = self.conn.lock().expect(LOCK_POISONED);
        let changed = conn.execute(
            "UPDATE clip SET pinned = ?1 WHERE id = ?2",
            params![pinned as i64, id.to_string()],
        )?;
        if changed == 0 {
            return Err(Error::NotFound(id));
        }
        Ok(())
    }

    /// Tombstones a clip: `deleted` is set and it drops out of the FTS index
    /// immediately, but the row itself survives so `changes_since` can still
    /// replicate the deletion to peers. Idempotent -- deleting an
    /// already-deleted (or already-purged) id is not an error, since a peer
    /// retrying after a lost acknowledgement must not see one.
    pub fn delete(&self, id: ClipId) -> Result<()> {
        let mut conn = self.conn.lock().expect(LOCK_POISONED);
        let tx = conn.transaction()?;
        let id_str = id.to_string();

        let changed = tx.execute(
            "UPDATE clip SET deleted = 1, deleted_at_ms = ?1 WHERE id = ?2 AND deleted = 0",
            params![current_epoch_ms() as i64, id_str],
        )?;

        if changed > 0 {
            tx.execute("DELETE FROM clip_fts WHERE clip_id = ?1", params![id_str])?;
        } else {
            let known: bool = tx
                .query_row("SELECT 1 FROM clip WHERE id = ?1", params![id_str], |_| {
                    Ok(true)
                })
                .optional()?
                .unwrap_or(false);
            if !known {
                return Err(Error::NotFound(id));
            }
            // Already a tombstone -- nothing left to do.
        }

        tx.commit()?;
        Ok(())
    }

    /// Physically removes tombstones older than `older_than_ms` (an age, not
    /// an absolute timestamp: a tombstone qualifies once `now - deleted_at`
    /// exceeds it). `payload` rows go with them via `ON DELETE CASCADE`, and
    /// any blob left with no remaining referencing payload is pruned too.
    pub fn purge_tombstones(&self, older_than_ms: u64) -> Result<usize> {
        let mut conn = self.conn.lock().expect(LOCK_POISONED);
        let tx = conn.transaction()?;
        let cutoff = current_epoch_ms().saturating_sub(older_than_ms) as i64;

        let purged = tx.execute(
            "DELETE FROM clip
             WHERE deleted = 1 AND deleted_at_ms IS NOT NULL AND deleted_at_ms <= ?1",
            params![cutoff],
        )?;

        crate::blob::prune_orphan_blobs(&tx, &self.blobs_root)?;
        tx.commit()?;
        Ok(purged)
    }

    /// Clips whose `Hlc` is strictly greater than `hlc` (or every clip, if
    /// `None`), oldest-first, tombstones included -- the sync engine relies
    /// on seeing deletions. "Strictly greater" (exclusive of the cursor
    /// itself) means a caller can pass back the last `Hlc` it received and
    /// never see that event redelivered, without needing a separate
    /// "have I already seen this one" check.
    pub fn changes_since(&self, hlc: Option<Hlc>, limit: usize) -> Result<Vec<Clip>> {
        let conn = self.conn.lock().expect(LOCK_POISONED);
        let raws: Vec<RawClip> = match hlc {
            None => {
                let sql = format!(
                    "SELECT {CLIP_COLUMNS} FROM clip
                     ORDER BY hlc_wall_ms ASC, hlc_counter ASC, hlc_device ASC
                     LIMIT ?1"
                );
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt
                    .query_map(params![limit as i64], RawClip::from_row)?
                    .collect::<rusqlite::Result<_>>()?;
                drop(stmt);
                rows
            }
            Some(cursor) => {
                // Written as an explicit OR-chain (rather than a row-value
                // comparison) so the intent -- lexicographic order over the
                // three HLC fields -- is legible without knowing SQLite's row
                // value comparison rules.
                let sql = format!(
                    "SELECT {CLIP_COLUMNS} FROM clip
                     WHERE hlc_wall_ms > ?1
                        OR (hlc_wall_ms = ?1 AND hlc_counter > ?2)
                        OR (hlc_wall_ms = ?1 AND hlc_counter = ?2 AND hlc_device > ?3)
                     ORDER BY hlc_wall_ms ASC, hlc_counter ASC, hlc_device ASC
                     LIMIT ?4"
                );
                let mut stmt = conn.prepare(&sql)?;
                // `hlc_device` orders as text here, which only agrees with
                // `DeviceId`'s own `Ord` (over the UUID's bytes) because
                // `Uuid::to_string()` is a fixed-width, lowercase hex
                // encoding -- byte order and lexicographic order coincide.
                let rows = stmt
                    .query_map(
                        params![
                            cursor.wall_ms as i64,
                            cursor.counter as i64,
                            cursor.device.to_string(),
                            limit as i64
                        ],
                        RawClip::from_row,
                    )?
                    .collect::<rusqlite::Result<_>>()?;
                drop(stmt);
                rows
            }
        };
        rows_to_clips(&conn, raws)
    }

    /// The newest `Hlc` this store has recorded (tombstones included), used
    /// by the sync engine to know what it has to offer a peer.
    pub fn max_hlc(&self) -> Result<Option<Hlc>> {
        let conn = self.conn.lock().expect(LOCK_POISONED);
        let row: Option<(i64, i64, String)> = conn
            .query_row(
                "SELECT hlc_wall_ms, hlc_counter, hlc_device FROM clip
                 ORDER BY hlc_wall_ms DESC, hlc_counter DESC, hlc_device DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        match row {
            None => Ok(None),
            Some((wall_ms, counter, device)) => {
                let device = device.parse()?;
                Ok(Some(Hlc::new(wall_ms as u64, counter as u32, device)))
            }
        }
    }

    /// Live clips, tombstones excluded — this is the number the UI shows, and
    /// a deleted clip is not something the user still has.
    pub fn clip_count(&self) -> Result<u64> {
        let conn = self.conn.lock().expect(LOCK_POISONED);
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM clip WHERE deleted = 0", [], |row| {
                row.get(0)
            })?;
        Ok(count.max(0) as u64)
    }

    /// Bytes currently held by the blob store, for the quota readout. Reads
    /// the bookkeeping table rather than walking the directory: the numbers
    /// have to agree with what `enforce_blob_quota` acts on.
    pub fn blob_bytes(&self) -> Result<u64> {
        let conn = self.conn.lock().expect(LOCK_POISONED);
        let total: i64 =
            conn.query_row("SELECT COALESCE(SUM(size), 0) FROM blob_meta", [], |row| {
                row.get(0)
            })?;
        Ok(total.max(0) as u64)
    }
}

fn load_clip(conn: &Connection, id: &str) -> Result<Option<Clip>> {
    let raw: Option<RawClip> = conn
        .query_row(
            &format!("SELECT {CLIP_COLUMNS} FROM clip WHERE id = ?1"),
            params![id],
            RawClip::from_row,
        )
        .optional()?;
    let Some(raw) = raw else { return Ok(None) };
    let payloads = load_payloads(conn, id)?;
    Ok(Some(raw.into_clip(payloads)?))
}

/// Loaded per clip id rather than in one join against `clip` -- this store
/// serves a local single-user history, not a high-QPS service, and keeping
/// row-mapping simple (one payload shape, one clip shape) was judged worth
/// more here than collapsing to a single round trip.
fn load_payloads(conn: &Connection, clip_id: &str) -> Result<Vec<clipse_core::Payload>> {
    let mut stmt = conn.prepare(
        "SELECT format_label, digest, size, inline_bytes FROM payload
         WHERE clip_id = ?1 ORDER BY format_label ASC",
    )?;
    stmt.query_map(params![clip_id], RawPayload::from_row)?
        .map(|raw| -> Result<clipse_core::Payload> { raw?.into_payload() })
        .collect()
}

fn rows_to_clips(conn: &Connection, raws: Vec<RawClip>) -> Result<Vec<Clip>> {
    raws.into_iter()
        .map(|raw| {
            let payloads = load_payloads(conn, &raw.id)?;
            raw.into_clip(payloads)
        })
        .collect()
}

fn current_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
