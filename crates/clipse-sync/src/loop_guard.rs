//! Stops a clip from bouncing between devices forever.
//!
//! A clip that arrives from a peer is written to the local clipboard, and the
//! local watcher then observes that write as a fresh copy. Without a guard,
//! that observation would be broadcast straight back and the two devices would
//! trade the same clip until one of them was closed.
//!
//! `clipse-clipboard` has its own guard for the write it just made; this one is
//! the second, independent layer, because platform clipboards are unreliable
//! narrators and a single dropped notification would otherwise turn into an
//! infinite loop. The third layer is content-addressed insert in the store,
//! where a bounced clip is a dedup no-op.
//!
//! # Scope: live broadcast only
//!
//! This governs the *push* that follows a local capture. It deliberately does
//! not touch reconciliation: when a device that was offline reconnects, it
//! catches up through `Store::changes_since` and the merge rules, so
//! suppressing a re-broadcast here can never cost anyone a clip. Every device
//! is paired with every other device, so the originator already sent it to
//! everyone that was reachable.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clipse_core::{ContentHash, DeviceId};

/// How long a hash received from a peer stays "recently seen".
///
/// Long enough to cover a slow clipboard notification (a busy machine can take
/// a second or two to deliver `WM_CLIPBOARDUPDATE`), short enough that
/// genuinely re-copying the same text half a minute later still syncs.
pub const DEFAULT_ECHO_WINDOW: Duration = Duration::from_secs(30);

/// Cap on remembered hashes. A pathological run of copies must not grow this
/// map without bound; entries are pruned by age first and this is the backstop.
const MAX_ENTRIES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebroadcastVerdict {
    Send,
    /// This content came from the very device we were about to send it to.
    SuppressOrigin,
    /// We received this content from some peer moments ago, so this local
    /// capture is the echo of our own write to the clipboard.
    SuppressEcho,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    origin: DeviceId,
    at_ms: u64,
}

pub struct LoopGuard {
    window_ms: u64,
    seen: HashMap<ContentHash, Entry>,
    /// Injectable for tests; production uses the system clock.
    now_ms: fn() -> u64,
}

impl LoopGuard {
    pub fn new(window: Duration) -> Self {
        Self {
            window_ms: window.as_millis() as u64,
            seen: HashMap::new(),
            now_ms: system_now_ms,
        }
    }

    #[cfg(test)]
    fn with_clock(window: Duration, now_ms: fn() -> u64) -> Self {
        Self {
            window_ms: window.as_millis() as u64,
            seen: HashMap::new(),
            now_ms,
        }
    }

    /// Note that `hash` arrived from `origin`. Called for every clip accepted
    /// from a peer, before it is written to the local clipboard.
    pub fn record_received(&mut self, hash: ContentHash, origin: DeviceId) {
        let now = (self.now_ms)();
        self.prune(now);
        self.seen.insert(hash, Entry { origin, at_ms: now });
    }

    /// Should a locally captured clip be pushed to `peer`?
    pub fn verdict(&self, hash: &ContentHash, peer: DeviceId) -> RebroadcastVerdict {
        let Some(entry) = self.seen.get(hash) else {
            return RebroadcastVerdict::Send;
        };

        let now = (self.now_ms)();
        if now.saturating_sub(entry.at_ms) > self.window_ms {
            return RebroadcastVerdict::Send;
        }

        // Checked first so the reason we report is the specific one: the peer
        // we are about to talk to is where this came from.
        if entry.origin == peer {
            RebroadcastVerdict::SuppressOrigin
        } else {
            RebroadcastVerdict::SuppressEcho
        }
    }

    /// Drop entries that have aged out, and hard-cap the map.
    fn prune(&mut self, now: u64) {
        let window = self.window_ms;
        self.seen
            .retain(|_, entry| now.saturating_sub(entry.at_ms) <= window);

        if self.seen.len() >= MAX_ENTRIES {
            // Keep the newest half rather than clearing outright: dropping
            // everything would briefly disable the guard, which is the one
            // thing it must never do.
            let mut by_age: Vec<(ContentHash, u64)> =
                self.seen.iter().map(|(h, e)| (*h, e.at_ms)).collect();
            by_age.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
            let keep: std::collections::HashSet<ContentHash> = by_age
                .into_iter()
                .take(MAX_ENTRIES / 2)
                .map(|(h, _)| h)
                .collect();
            self.seen.retain(|hash, _| keep.contains(hash));
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.seen.len()
    }
}

impl Default for LoopGuard {
    fn default() -> Self {
        Self::new(DEFAULT_ECHO_WINDOW)
    }
}

fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    // The clock has to be reachable from a bare `fn() -> u64`, which cannot
    // capture per-instance state — so it lives outside the guard. It is
    // thread-local rather than process-wide because `cargo test` runs each
    // `#[test]` on its own thread: with one shared clock, a test that advanced
    // time moved it under whatever a sibling test was measuring, and the suite
    // failed at random under `--workspace` parallelism.
    thread_local! {
        static FAKE_NOW: Cell<u64> = const { Cell::new(0) };
    }

    fn fake_now() -> u64 {
        FAKE_NOW.get()
    }

    fn set_now(ms: u64) {
        FAKE_NOW.set(ms);
    }

    fn guard() -> LoopGuard {
        set_now(1_000_000);
        LoopGuard::with_clock(Duration::from_secs(30), fake_now)
    }

    #[test]
    fn a_clip_we_never_saw_is_sent() {
        let guard = guard();
        let hash = ContentHash::of(b"fresh local copy");
        assert_eq!(
            guard.verdict(&hash, DeviceId::generate()),
            RebroadcastVerdict::Send
        );
    }

    #[test]
    fn a_clip_is_never_sent_back_to_where_it_came_from() {
        let mut guard = guard();
        let peer = DeviceId::generate();
        let hash = ContentHash::of(b"from the laptop");
        guard.record_received(hash, peer);

        assert_eq!(
            guard.verdict(&hash, peer),
            RebroadcastVerdict::SuppressOrigin
        );
    }

    #[test]
    fn the_echo_of_our_own_write_is_not_broadcast_to_anyone() {
        let mut guard = guard();
        let sender = DeviceId::generate();
        let third_device = DeviceId::generate();
        let hash = ContentHash::of(b"arrived from a peer");
        guard.record_received(hash, sender);

        // Our clipboard watcher will see the write we just made. Pushing that
        // to a third device would be the same content going round again.
        assert_eq!(
            guard.verdict(&hash, third_device),
            RebroadcastVerdict::SuppressEcho
        );
    }

    #[test]
    fn a_genuine_re_copy_after_the_window_is_sent() {
        let mut guard = guard();
        let peer = DeviceId::generate();
        let hash = ContentHash::of(b"the same text again");
        guard.record_received(hash, peer);
        assert_eq!(
            guard.verdict(&hash, peer),
            RebroadcastVerdict::SuppressOrigin
        );

        // Half a minute later the user really did copy it again.
        set_now(1_000_000 + 31_000);
        assert_eq!(guard.verdict(&hash, peer), RebroadcastVerdict::Send);
    }

    #[test]
    fn different_content_is_unaffected() {
        let mut guard = guard();
        let peer = DeviceId::generate();
        guard.record_received(ContentHash::of(b"one thing"), peer);

        assert_eq!(
            guard.verdict(&ContentHash::of(b"another thing"), peer),
            RebroadcastVerdict::Send
        );
    }

    #[test]
    fn aged_out_entries_are_pruned() {
        let mut guard = guard();
        let peer = DeviceId::generate();
        guard.record_received(ContentHash::of(b"old"), peer);
        assert_eq!(guard.len(), 1);

        set_now(1_000_000 + 60_000);
        guard.record_received(ContentHash::of(b"new"), peer);
        assert_eq!(guard.len(), 1, "the aged entry should have been dropped");
    }

    #[test]
    fn the_map_is_bounded_and_keeps_the_newest() {
        let mut guard = guard();
        let peer = DeviceId::generate();

        // All within the window, so age-based pruning cannot help — this is
        // the hard cap doing the work.
        for i in 0..(MAX_ENTRIES + 100) {
            set_now(1_000_000 + i as u64);
            guard.record_received(ContentHash::of(&i.to_le_bytes()), peer);
        }

        assert!(
            guard.len() <= MAX_ENTRIES,
            "loop guard grew past its cap: {}",
            guard.len()
        );

        // The most recent hash must still be guarded — losing that is exactly
        // when a loop would start.
        let newest = ContentHash::of(&(MAX_ENTRIES + 99).to_le_bytes());
        assert_eq!(
            guard.verdict(&newest, peer),
            RebroadcastVerdict::SuppressOrigin
        );
    }

    #[test]
    fn a_clip_bouncing_between_two_devices_stops_after_one_hop() {
        // Device A copies; B receives and writes it to its clipboard; B's
        // watcher observes that write. B must not send it back.
        let device_a = DeviceId::generate();
        let mut on_b = guard();

        let hash = ContentHash::of(b"round trip");
        on_b.record_received(hash, device_a);

        assert_ne!(on_b.verdict(&hash, device_a), RebroadcastVerdict::Send);
    }
}
