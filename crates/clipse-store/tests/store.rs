//! Integration tests against the public `Store` API. Each test owns a fresh
//! `tempfile::tempdir()` so they can run in parallel without contending on
//! one SQLite file.

use std::sync::Arc;
use std::thread;

use clipse_core::{
    Clip, ClipFormat, ClipKind, ClipSource, ContentHash, DeviceId, Hlc, INLINE_MAX_BYTES, Paths,
    Payload,
};
use clipse_store::{Error, HistoryQuery, InsertOutcome, Store, StoreOptions};
use tempfile::TempDir;

fn open_store() -> (TempDir, Store) {
    let dir = TempDir::new().expect("tempdir");
    let paths = Paths::with_root(dir.path());
    let store = Store::open(&paths, StoreOptions::default()).expect("open store");
    (dir, store)
}

fn source() -> ClipSource {
    ClipSource::new(DeviceId::generate(), "test-device").with_app(Some("test-app".into()))
}

fn hlc_at(wall_ms: u64, device: DeviceId) -> Hlc {
    Hlc::new(wall_ms, 0, device)
}

fn text_clip(text: &str, hlc: Hlc) -> Clip {
    Clip::new(
        vec![Payload::new(ClipFormat::Text, text.as_bytes().to_vec())],
        source(),
        hlc,
    )
}

// --- 1. round trip -----------------------------------------------------

#[test]
fn insert_then_get_by_hash_and_recent_all_agree() {
    let (_dir, store) = open_store();
    let device = DeviceId::generate();
    let clip = Clip::new(
        vec![
            Payload::new(ClipFormat::Text, b"hello there".to_vec()),
            Payload::new(ClipFormat::Html, b"<b>hello there</b>".to_vec()),
        ],
        source(),
        hlc_at(1_000, device),
    );

    let outcome = store.insert(&clip).unwrap();
    assert_eq!(outcome, InsertOutcome::Inserted(clip.id));

    let via_get = store.get(clip.id).unwrap().expect("row must exist");
    assert_eq!(via_get, clip);

    let via_hash = store.by_hash(&clip.hash).unwrap().expect("row must exist");
    assert_eq!(via_hash, clip);

    let recent = store
        .recent(HistoryQuery {
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(recent, vec![clip]);
}

#[test]
fn pinned_flag_round_trips() {
    let (_dir, store) = open_store();
    let clip = text_clip("pin me", hlc_at(1_000, DeviceId::generate()));
    store.insert(&clip).unwrap();

    let pin_hlc = hlc_at(9_999, DeviceId::generate());
    store.set_pinned(clip.id, true, pin_hlc).unwrap();

    let pinned = store.get(clip.id).unwrap().unwrap();
    assert!(pinned.pinned);
    // The clock has to move, or the change could never replicate to another
    // device -- every merge rule and sync cursor keys on the HLC.
    assert_eq!(pinned.hlc, pin_hlc);
    // Everything that is not metadata is untouched.
    assert_eq!(pinned.payloads, clip.payloads);
    assert_eq!(pinned.created_at_ms, clip.created_at_ms);

    let unpin_hlc = hlc_at(10_000, DeviceId::generate());
    store.set_pinned(clip.id, false, unpin_hlc).unwrap();
    let unpinned = store.get(clip.id).unwrap().unwrap();
    assert!(!unpinned.pinned);
    assert_eq!(unpinned.hlc, unpin_hlc);
}

#[test]
fn set_pinned_on_unknown_id_errors() {
    let (_dir, store) = open_store();
    let err = store
        .set_pinned(
            clipse_core::ClipId::generate(),
            true,
            hlc_at(1, DeviceId::generate()),
        )
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

// --- 2. dedup ------------------------------------------------------------

#[test]
fn duplicate_content_dedups_and_floats_to_top() {
    let (_dir, store) = open_store();
    let device = DeviceId::generate();

    let first = text_clip("same content", hlc_at(1_000, device));
    assert_eq!(
        store.insert(&first).unwrap(),
        InsertOutcome::Inserted(first.id)
    );

    let other = text_clip("different content", hlc_at(2_000, device));
    assert_eq!(
        store.insert(&other).unwrap(),
        InsertOutcome::Inserted(other.id)
    );

    // Re-"copy" the same content later: same hash, new capture event.
    let repeat = text_clip("same content", hlc_at(3_000, device));
    assert_eq!(repeat.hash, first.hash);
    let outcome = store.insert(&repeat).unwrap();
    assert_eq!(
        outcome,
        InsertOutcome::Deduplicated(first.id),
        "must keep the original id"
    );

    // Exactly one row for the deduplicated content.
    let recent = store
        .recent(HistoryQuery {
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(recent.len(), 2, "dedup must not create a second row");
    assert_eq!(
        recent[0].id, first.id,
        "the re-copied clip floats to the top"
    );
    assert_eq!(
        recent[0].hlc, repeat.hlc,
        "recency is bumped to the new event"
    );
    assert_eq!(recent[1].id, other.id);
}

#[test]
fn dedup_does_not_disturb_pinned_state() {
    let (_dir, store) = open_store();
    let device = DeviceId::generate();
    let first = text_clip("keep me pinned", hlc_at(1_000, device));
    store.insert(&first).unwrap();
    store
        .set_pinned(first.id, true, hlc_at(9_999, DeviceId::generate()))
        .unwrap();

    let repeat = text_clip("keep me pinned", hlc_at(2_000, device));
    store.insert(&repeat).unwrap();

    assert!(
        store.get(first.id).unwrap().unwrap().pinned,
        "dedup must not un-pin"
    );
}

// --- 3. tamper rejection --------------------------------------------------

#[test]
fn tampered_hash_is_rejected() {
    let (_dir, store) = open_store();
    let mut clip = text_clip("original", hlc_at(1_000, DeviceId::generate()));
    // Swap the payload without updating `hash` -- simulates a forged or
    // corrupted clip arriving from a peer.
    clip.payloads[0] = Payload::new(ClipFormat::Text, b"forged".to_vec());
    assert!(!clip.hash_matches());

    let outcome = store.insert(&clip).unwrap();
    assert_eq!(outcome, InsertOutcome::Rejected);
    assert_eq!(
        store.get(clip.id).unwrap(),
        None,
        "a rejected clip must not be persisted"
    );
}

// --- 4. FTS ---------------------------------------------------------------

#[test]
fn search_finds_a_word_in_the_middle_of_the_text() {
    let (_dir, store) = open_store();
    let clip = text_clip(
        "the quick brown needle jumps over",
        hlc_at(1_000, DeviceId::generate()),
    );
    store.insert(&clip).unwrap();

    let hits = store.search("needle", HistoryQuery::default()).unwrap();
    assert_eq!(hits, vec![clip]);
}

#[test]
fn deleted_clip_stops_appearing_in_search() {
    let (_dir, store) = open_store();
    let clip = text_clip("findable text", hlc_at(1_000, DeviceId::generate()));
    store.insert(&clip).unwrap();
    assert_eq!(
        store
            .search("findable", HistoryQuery::default())
            .unwrap()
            .len(),
        1
    );

    store
        .delete(clip.id, hlc_at(9_999, DeviceId::generate()))
        .unwrap();
    assert!(
        store
            .search("findable", HistoryQuery::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reinserted_content_leaves_no_stale_index_row() {
    let (_dir, store) = open_store();
    let device = DeviceId::generate();

    let v1 = text_clip("version one alpha", hlc_at(1_000, device));
    store.insert(&v1).unwrap();
    store
        .delete(v1.id, hlc_at(9_999, DeviceId::generate()))
        .unwrap();

    // A different clip re-using the same conceptual "slot" (e.g. an edit)
    // is, from the store's point of view, an unrelated fresh clip because
    // its content -- and therefore its hash -- differs.
    let v2 = text_clip("version two beta", hlc_at(2_000, device));
    store.insert(&v2).unwrap();

    assert!(
        store
            .search("alpha", HistoryQuery::default())
            .unwrap()
            .is_empty()
    );
    let hits = store.search("beta", HistoryQuery::default()).unwrap();
    assert_eq!(hits, vec![v2]);
}

#[test]
fn search_query_syntax_characters_are_safe() {
    let (_dir, store) = open_store();
    let device = DeviceId::generate();
    let literal_and = text_clip("look AND behold", hlc_at(1_000, device));
    let literal_star = text_clip("wildcard * marks the spot", hlc_at(2_000, device));
    let literal_quote = text_clip("she said foo\"bar to me", hlc_at(3_000, device));
    store.insert(&literal_and).unwrap();
    store.insert(&literal_star).unwrap();
    store.insert(&literal_quote).unwrap();

    // None of these panic or return an FTS5 syntax error -- and each is
    // treated as literal text, not as a query operator.
    assert_eq!(
        store.search("AND", HistoryQuery::default()).unwrap(),
        vec![literal_and]
    );
    assert_eq!(
        store.search("foo\"bar", HistoryQuery::default()).unwrap(),
        vec![literal_quote]
    );
    // "*" is not itself a word character under FTS5's default tokenizer, so
    // no document ever indexes a "*" token -- searching for it can never
    // match, escaped or not. The safety property under test is that it does
    // not error out or get treated as the unescaped prefix-wildcard
    // operator (which would otherwise turn `* ` into a syntax error).
    assert!(
        store
            .search("*", HistoryQuery::default())
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .search("", HistoryQuery::default())
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .search("   ", HistoryQuery::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn history_query_filters_by_kind_and_pinned() {
    let (_dir, store) = open_store();
    let device = DeviceId::generate();
    let text = text_clip("plain text clip", hlc_at(1_000, device));
    let image = Clip::new(
        vec![Payload::new(ClipFormat::Png, vec![1, 2, 3, 4])],
        source(),
        hlc_at(2_000, device),
    );
    store.insert(&text).unwrap();
    store.insert(&image).unwrap();
    store
        .set_pinned(text.id, true, hlc_at(9_999, DeviceId::generate()))
        .unwrap();

    let images = store
        .recent(HistoryQuery {
            kind: Some(ClipKind::Image),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].id, image.id);

    let pinned = store
        .recent(HistoryQuery {
            pinned_only: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].id, text.id);
}

// --- 5. blob storage -------------------------------------------------------

#[test]
fn large_payload_round_trips_through_the_blob_store() {
    let (_dir, store) = open_store();
    let big_bytes = vec![0x42u8; (INLINE_MAX_BYTES + 1) as usize];
    let payload = Payload::new(ClipFormat::Png, big_bytes.clone());
    assert!(payload.is_blob());
    let digest = payload.digest;

    let clip = Clip::new(vec![payload], source(), hlc_at(1_000, DeviceId::generate()));
    store.insert(&clip).unwrap();

    // Not written to the blob store yet: the clip is known but incomplete.
    assert!(!store.has_blob(&digest).unwrap());
    let reloaded = store.get(clip.id).unwrap().unwrap();
    assert!(!reloaded.is_complete(|d| store.has_blob(d).unwrap()));

    store.put_blob(&digest, &big_bytes).unwrap();
    assert!(store.has_blob(&digest).unwrap());
    assert_eq!(store.read_blob(&digest).unwrap(), big_bytes);

    let reloaded = store.get(clip.id).unwrap().unwrap();
    assert_eq!(reloaded, clip);
    assert!(reloaded.is_complete(|d| store.has_blob(d).unwrap()));
}

#[test]
fn small_payload_stays_inline() {
    let (_dir, store) = open_store();
    let bytes = vec![0x7u8; 10];
    let payload = Payload::new(ClipFormat::Png, bytes.clone());
    assert!(!payload.is_blob());
    let digest = payload.digest;

    let clip = Clip::new(vec![payload], source(), hlc_at(1_000, DeviceId::generate()));
    store.insert(&clip).unwrap();

    assert!(
        !store.has_blob(&digest).unwrap(),
        "small payloads never touch the blob store"
    );
    let reloaded = store.get(clip.id).unwrap().unwrap();
    assert_eq!(
        reloaded.payload(&ClipFormat::Png).unwrap().inline_bytes(),
        Some(bytes.as_slice())
    );
}

#[test]
fn read_blob_of_unknown_digest_errors() {
    let (_dir, store) = open_store();
    let digest = ContentHash::of(b"never written");
    let err = store.read_blob(&digest).unwrap_err();
    assert!(matches!(err, Error::BlobNotFound(_)));
}

// --- 6. quota ---------------------------------------------------------------

fn blob_clip(seed: u8, hlc: Hlc) -> (Clip, Vec<u8>) {
    // Slightly different size per seed keeps each digest distinct even
    // though every byte in a given blob is otherwise identical.
    let size = INLINE_MAX_BYTES as usize + 6_000 - seed as usize;
    let bytes = vec![seed; size];
    let payload = Payload::new(ClipFormat::Png, bytes.clone());
    let clip = Clip::new(vec![payload], source(), hlc);
    (clip, bytes)
}

#[test]
fn quota_evicts_lru_blobs_but_never_pinned_or_inline() {
    let dir = TempDir::new().unwrap();
    let paths = Paths::with_root(dir.path());
    // Each blob below is a little over INLINE_MAX_BYTES; three of them
    // comfortably exceed this quota, forcing exactly one eviction.
    let quota = ((INLINE_MAX_BYTES as usize + 6_000) * 2 + 1_000) as u64;
    // `with_quota` rather than a struct literal so this test compiles both
    // with and without the `encryption` feature, which adds a field.
    let store = Store::open(&paths, StoreOptions::with_quota(quota)).unwrap();

    let device = DeviceId::generate();
    let (pinned_clip, pinned_bytes) = blob_clip(1, hlc_at(1_000, device));
    let (old_clip, old_bytes) = blob_clip(2, hlc_at(2_000, device));
    let (new_clip, new_bytes) = blob_clip(3, hlc_at(3_000, device));
    let inline_clip = text_clip("small clip untouched by quota", hlc_at(4_000, device));

    store.insert(&pinned_clip).unwrap();
    store
        .put_blob(&pinned_clip.payloads[0].digest, &pinned_bytes)
        .unwrap(); // used_seq 1 (oldest)
    store.insert(&old_clip).unwrap();
    store
        .put_blob(&old_clip.payloads[0].digest, &old_bytes)
        .unwrap(); // used_seq 2
    store.insert(&new_clip).unwrap();
    store
        .put_blob(&new_clip.payloads[0].digest, &new_bytes)
        .unwrap(); // used_seq 3 (newest)
    store.insert(&inline_clip).unwrap();

    // Pin *after* writing its blob: despite being the least-recently-used
    // by used_seq, it must survive because it is pinned.
    store
        .set_pinned(pinned_clip.id, true, hlc_at(9_999, DeviceId::generate()))
        .unwrap();

    let report = store.enforce_blob_quota().unwrap();
    assert_eq!(
        report.blobs_evicted, 1,
        "exactly one blob should be evicted: {report:?}"
    );
    assert!(
        report.bytes_after <= quota,
        "must be back under quota: {report:?}"
    );

    // Never deletes a blob belonging to a pinned clip, even though it was
    // the oldest by LRU order.
    assert!(store.has_blob(&pinned_clip.payloads[0].digest).unwrap());
    // The least-recently-used *unpinned* blob is the one that goes.
    assert!(!store.has_blob(&old_clip.payloads[0].digest).unwrap());
    // A more-recently-used unpinned blob is spared.
    assert!(store.has_blob(&new_clip.payloads[0].digest).unwrap());

    // The clip row survives eviction; only completeness changes.
    let reloaded_old = store
        .get(old_clip.id)
        .unwrap()
        .expect("clip row must survive eviction");
    assert_eq!(
        reloaded_old.payloads, old_clip.payloads,
        "payload row is untouched, only bytes"
    );
    assert!(!reloaded_old.is_complete(|d| store.has_blob(d).unwrap()));

    let reloaded_pinned = store.get(pinned_clip.id).unwrap().unwrap();
    assert!(reloaded_pinned.is_complete(|d| store.has_blob(d).unwrap()));
    let reloaded_new = store.get(new_clip.id).unwrap().unwrap();
    assert!(reloaded_new.is_complete(|d| store.has_blob(d).unwrap()));

    // Never touches an inline (non-blob) payload.
    assert_eq!(store.get(inline_clip.id).unwrap().unwrap(), inline_clip);
}

// --- 7. sync cursor ----------------------------------------------------------

#[test]
fn changes_since_orders_by_hlc_and_is_cursor_exclusive() {
    let (_dir, store) = open_store();
    let device = DeviceId::generate();
    let a = text_clip("a", hlc_at(1_000, device));
    let b = text_clip("b", hlc_at(2_000, device));
    let c = text_clip("c", hlc_at(3_000, device));
    // Insert out of hlc order to prove ordering comes from the hlc, not
    // insertion order.
    store.insert(&c).unwrap();
    store.insert(&a).unwrap();
    store.insert(&b).unwrap();

    let all = store.changes_since(None, 10).unwrap();
    assert_eq!(
        all.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![a.id, b.id, c.id]
    );

    let since_a = store.changes_since(Some(a.hlc), 10).unwrap();
    assert_eq!(
        since_a.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![b.id, c.id],
        "cursor must be exclusive of the hlc passed in"
    );

    assert_eq!(store.max_hlc().unwrap(), Some(c.hlc));
}

#[test]
fn changes_since_includes_tombstones() {
    let (_dir, store) = open_store();
    let device = DeviceId::generate();
    let a = text_clip("a", hlc_at(1_000, device));
    let b = text_clip("b", hlc_at(2_000, device));
    store.insert(&a).unwrap();
    store.insert(&b).unwrap();
    store
        .delete(a.id, hlc_at(9_999, DeviceId::generate()))
        .unwrap();

    let all = store.changes_since(None, 10).unwrap();
    assert_eq!(all.len(), 2, "tombstones must still replicate");
    let tombstone = all
        .iter()
        .find(|c| c.id == a.id)
        .expect("tombstone present");
    assert!(tombstone.deleted, "sync engine needs to see the deletion");
}

#[test]
fn purge_tombstones_removes_only_sufficiently_old_ones() {
    let (_dir, store) = open_store();
    let device = DeviceId::generate();
    let a = text_clip("purge me", hlc_at(1_000, device));
    store.insert(&a).unwrap();
    store
        .delete(a.id, hlc_at(9_999, DeviceId::generate()))
        .unwrap();

    // Not old enough yet (a huge age threshold means "must be ancient").
    let purged = store.purge_tombstones(u64::MAX / 2).unwrap();
    assert_eq!(purged, 0);
    assert!(
        store.get(a.id).unwrap().is_some(),
        "tombstone still present before its time"
    );

    // An age of 0 means "anything already deleted qualifies".
    let purged = store.purge_tombstones(0).unwrap();
    assert_eq!(purged, 1);
    assert!(
        store.get(a.id).unwrap().is_none(),
        "purged tombstone is gone for good"
    );
}

// --- 8. migrations -------------------------------------------------------

#[test]
fn reopening_an_existing_database_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let paths = Paths::with_root(dir.path());
    let clip = text_clip(
        "persisted across reopen",
        hlc_at(1_000, DeviceId::generate()),
    );

    {
        let store = Store::open(&paths, StoreOptions::default()).unwrap();
        store.insert(&clip).unwrap();
    }
    {
        let store = Store::open(&paths, StoreOptions::default()).unwrap();
        assert_eq!(store.get(clip.id).unwrap(), Some(clip));
    }
}

#[test]
fn a_database_from_a_newer_schema_refuses_to_open() {
    let dir = TempDir::new().unwrap();
    let paths = Paths::with_root(dir.path());
    {
        let _store = Store::open(&paths, StoreOptions::default()).unwrap();
    }

    // Simulate a database written by some future version of this crate.
    {
        let conn = rusqlite::Connection::open(paths.database()).unwrap();
        conn.execute("UPDATE schema_version SET version = 999999", [])
            .unwrap();
    }

    let err = Store::open(&paths, StoreOptions::default())
        .err()
        .expect("must refuse to open");
    match err {
        Error::SchemaTooNew { found, supported } => {
            assert_eq!(found, 999_999);
            assert!(supported < found);
        }
        other => panic!("expected SchemaTooNew, got {other:?}"),
    }
}

// --- 9. concurrency -----------------------------------------------------

#[test]
fn concurrent_inserts_from_multiple_threads_do_not_corrupt_or_deadlock() {
    let (_dir, store) = open_store();
    let store = Arc::new(store);

    let threads: Vec<_> = (0..8u32)
        .map(|t| {
            let store = Arc::clone(&store);
            thread::spawn(move || {
                let device = DeviceId::generate();
                for i in 0..25u32 {
                    let clip = text_clip(
                        &format!("thread {t} clip {i}"),
                        hlc_at(1_000 + i as u64, device),
                    );
                    let outcome = store.insert(&clip).unwrap();
                    assert_eq!(outcome, InsertOutcome::Inserted(clip.id));
                }
            })
        })
        .collect();

    for handle in threads {
        handle.join().expect("worker thread must not panic");
    }

    let all = store
        .recent(HistoryQuery {
            limit: 1_000,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        all.len(),
        8 * 25,
        "every insert from every thread must be visible and distinct"
    );
}
