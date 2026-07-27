//! Forward-only schema migrations.
//!
//! `schema_version` holds exactly one row. Opening a database whose row is
//! behind `CURRENT_SCHEMA_VERSION` runs every migration between the two,
//! in order, inside one transaction. Opening one that is *ahead* fails with
//! [`crate::Error::SchemaTooNew`] rather than guessing how to downgrade.

use rusqlite::{Connection, OptionalExtension};

use crate::error::{Error, Result};

/// Bump this and append a migration function whenever the schema changes.
/// Never renumber or remove an existing entry — a device mid-upgrade may
/// still have an older row that expects the old sequence.
pub(crate) const CURRENT_SCHEMA_VERSION: i64 = 1;

type Migration = fn(&Connection) -> rusqlite::Result<()>;

const MIGRATIONS: &[(i64, Migration)] = &[(1, migrate_v1)];

pub(crate) fn open_and_migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);")?;

    let mut version: i64 = conn
        .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
            row.get(0)
        })
        .optional()?
        .unwrap_or(0);

    if version == 0 {
        conn.execute("INSERT INTO schema_version (version) VALUES (0)", [])?;
    }

    if version > CURRENT_SCHEMA_VERSION {
        return Err(Error::SchemaTooNew {
            found: version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }

    for (target, migrate) in MIGRATIONS {
        if *target > version {
            migrate(conn)?;
            conn.execute("UPDATE schema_version SET version = ?1", [target])?;
            version = *target;
        }
    }

    Ok(())
}

/// Base schema: clips, their payloads, blob LRU metadata and the FTS5 index.
///
/// `clip` and `payload` remain the source of truth; `clip_fts` is a
/// derived index kept in sync by explicit inserts/updates/deletes in the
/// Rust layer rather than SQL triggers. Triggers would still need the same
/// "what's the searchable text for this clip" logic the insert path already
/// has (which payload counts as body text, how previews fall back), so a
/// trigger and the application code would inevitably diverge over the format
/// list; one code path is easier to keep correct and to unit test.
fn migrate_v1(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE clip (
            id                    TEXT PRIMARY KEY,
            hash                  TEXT NOT NULL,
            kind                  TEXT NOT NULL,
            preview               TEXT NOT NULL,
            source_device         TEXT NOT NULL,
            source_device_label   TEXT NOT NULL,
            source_app            TEXT,
            hlc_wall_ms           INTEGER NOT NULL,
            hlc_counter           INTEGER NOT NULL,
            hlc_device            TEXT NOT NULL,
            created_at_ms         INTEGER NOT NULL,
            pinned                INTEGER NOT NULL DEFAULT 0,
            deleted               INTEGER NOT NULL DEFAULT 0,
            -- Set when `deleted` flips to 1; NULL for a live row. Distinct
            -- from `created_at_ms` (the clip's own content timestamp) and
            -- from the HLC (causal order, not wall-clock age) -- this is
            -- specifically the tombstone's own age, which is what
            -- `purge_tombstones` compares against its cutoff.
            deleted_at_ms         INTEGER
        ) STRICT;

        CREATE INDEX clip_hash_idx ON clip(hash) WHERE deleted = 0;
        CREATE INDEX clip_hlc_idx ON clip(hlc_wall_ms, hlc_counter, hlc_device);
        CREATE INDEX clip_recent_idx ON clip(deleted, pinned, hlc_wall_ms DESC);

        CREATE TABLE payload (
            clip_id       TEXT NOT NULL REFERENCES clip(id) ON DELETE CASCADE,
            format_label  TEXT NOT NULL,
            digest        TEXT NOT NULL,
            size          INTEGER NOT NULL,
            inline_bytes  BLOB,
            PRIMARY KEY (clip_id, format_label)
        ) STRICT;

        CREATE INDEX payload_digest_idx ON payload(digest);

        -- LRU bookkeeping for blobs actually written to disk. `used_seq` is a
        -- monotonic counter (not wall-clock time) so eviction order in tests
        -- does not depend on the OS clock's resolution.
        CREATE TABLE blob_meta (
            digest    TEXT PRIMARY KEY,
            size      INTEGER NOT NULL,
            used_seq  INTEGER NOT NULL
        ) STRICT;

        CREATE INDEX blob_meta_used_idx ON blob_meta(used_seq);

        -- Deliberately NOT `content=''` (contentless): a contentless FTS5
        -- table refuses to return even its UNINDEXED columns (they read back
        -- NULL), so we could not map a MATCH hit back to a clip_id. This
        -- table is a small standalone index instead, populated and pruned by
        -- the Rust layer alongside `clip`/`payload`.
        CREATE VIRTUAL TABLE clip_fts USING fts5(
            clip_id UNINDEXED,
            body
        );
        ",
    )
}
