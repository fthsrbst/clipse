use std::sync::Mutex;
use std::time::{Duration, Instant};

use clipse_core::ContentHash;

/// Breaks the write -> capture -> write loop: when this crate puts content on
/// the clipboard on behalf of a remote peer, the platform watcher's very next
/// notification is *our own write echoing back*, not a new local copy. Without
/// this, every incoming remote clip would immediately re-enter the sync engine
/// as if the user had copied it locally, and devices would ping-pong the same
/// clip forever.
///
/// Deliberately not platform-specific: it only ever sees content hashes, so it
/// is exercised by plain unit tests without touching an OS clipboard.
///
/// Expiry strategy: a single pending entry that is cleared on its **first
/// match** (the common case — the OS delivers the echo as the very next
/// change) **or** after a short TTL, whichever comes first. A pure
/// "first match, no timeout" guard would wait forever if a write's echo never
/// arrives (e.g. another process raced in and changed the clipboard again
/// before our own notification fired, or the write silently failed), leaving
/// a stale hash around that could suppress a *genuine* future re-copy of the
/// same content — the failure mode the task explicitly calls out. The TTL is
/// the backstop for that case. 750ms was picked because the slowest backend
/// (macOS, which polls `changeCount` every 250ms) needs up to one poll
/// interval plus scheduling slack to observe the echo at all; anything near
/// that poll period would be too tight.
pub struct OwnWriteGuard {
    ttl: Duration,
    pending: Mutex<Option<Pending>>,
    /// Injectable for tests; production uses a monotonic clock. A boxed
    /// closure (not a bare `fn() -> Instant`) so each test can carry its own
    /// independent, isolated offset instead of sharing one process-wide
    /// static — a bare fn pointer cannot close over per-test state, and a
    /// shared static would make parallel tests race on each other's clock.
    now: Box<dyn Fn() -> Instant + Send + Sync>,
}

struct Pending {
    hash: ContentHash,
    written_at: Instant,
}

/// Default TTL — see the doc comment on `OwnWriteGuard` for why 750ms.
pub const DEFAULT_OWN_WRITE_TTL: Duration = Duration::from_millis(750);

impl OwnWriteGuard {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            pending: Mutex::new(None),
            now: Box::new(Instant::now),
        }
    }

    #[cfg(test)]
    fn with_clock(ttl: Duration, now: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        Self {
            ttl,
            pending: Mutex::new(None),
            now: Box::new(now),
        }
    }

    /// Call immediately after a successful `Clipboard::write`.
    ///
    /// Overwrites any prior pending entry: only one write can be "in flight"
    /// waiting for its echo at a time, and a newer write's echo is what we
    /// actually expect next.
    pub fn record_write(&self, hash: ContentHash) {
        let mut pending = self.pending.lock().expect("own-write guard mutex poisoned");
        *pending = Some(Pending {
            hash,
            written_at: (self.now)(),
        });
    }

    /// Call for every capture the watcher observes. Returns `true` when the
    /// capture is our own echo (and should be suppressed with
    /// `SuppressionReason::OwnWrite`).
    ///
    /// The entry is only removed on a match or on expiry — an unrelated
    /// capture that arrives first (a miss) leaves it in place, because the
    /// real echo of our write may still show up right after it.
    pub fn check(&self, hash: ContentHash) -> bool {
        let mut pending = self.pending.lock().expect("own-write guard mutex poisoned");
        let Some(p) = pending.as_ref() else {
            return false;
        };

        if (self.now)().duration_since(p.written_at) > self.ttl {
            *pending = None;
            return false;
        }
        if p.hash == hash {
            *pending = None; // consumed: this capture was the echo
            true
        } else {
            false
        }
    }
}

impl Default for OwnWriteGuard {
    fn default() -> Self {
        Self::new(DEFAULT_OWN_WRITE_TTL)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// A fake clock with its own offset, isolated per test so parallel test
    /// threads cannot race on each other's notion of "now".
    #[derive(Clone)]
    struct FakeClock {
        base: Instant,
        offset_ms: Arc<AtomicU64>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                base: Instant::now(),
                offset_ms: Arc::new(AtomicU64::new(0)),
            }
        }

        fn advance(&self, ms: u64) {
            self.offset_ms.fetch_add(ms, Ordering::SeqCst);
        }

        fn now(&self) -> Instant {
            self.base + Duration::from_millis(self.offset_ms.load(Ordering::SeqCst))
        }

        fn guard(&self, ttl: Duration) -> OwnWriteGuard {
            let clock = self.clone();
            OwnWriteGuard::with_clock(ttl, move || clock.now())
        }
    }

    fn hash(bytes: &[u8]) -> ContentHash {
        ContentHash::of(bytes)
    }

    #[test]
    fn echo_of_own_write_is_suppressed() {
        let guard = FakeClock::new().guard(Duration::from_millis(500));
        let h = hash(b"remote clip");
        guard.record_write(h);
        assert!(guard.check(h), "the write's own echo must be suppressed");
    }

    #[test]
    fn only_the_first_matching_capture_is_suppressed() {
        let guard = FakeClock::new().guard(Duration::from_millis(500));
        let h = hash(b"remote clip");
        guard.record_write(h);
        assert!(guard.check(h), "first capture is the echo");
        assert!(
            !guard.check(h),
            "a second identical capture is a genuine re-copy"
        );
    }

    #[test]
    fn unrelated_capture_in_between_does_not_consume_the_guard() {
        let guard = FakeClock::new().guard(Duration::from_millis(500));
        let ours = hash(b"remote clip");
        let other = hash(b"something the user copied moments later");
        guard.record_write(ours);
        assert!(!guard.check(other), "unrelated capture must pass through");
        assert!(
            guard.check(ours),
            "the real echo can still arrive after the miss"
        );
    }

    #[test]
    fn stale_entry_expires_after_ttl_so_a_real_recopy_is_not_suppressed_forever() {
        let clock = FakeClock::new();
        let guard = clock.guard(Duration::from_millis(500));
        let h = hash(b"clip content");
        guard.record_write(h);
        clock.advance(600); // past the 500ms TTL, echo apparently never arrived
        assert!(
            !guard.check(h),
            "expired entry must not suppress a later genuine copy"
        );
    }

    #[test]
    fn no_pending_write_never_suppresses() {
        let guard = FakeClock::new().guard(Duration::from_millis(500));
        assert!(!guard.check(hash(b"anything")));
    }
}
