//! Hybrid logical clocks.
//!
//! Clipse has no server, so "which copy is newer" cannot rely on wall clocks
//! that disagree between a laptop and a desktop. An HLC keeps timestamps close
//! to physical time (so the history reads sensibly) while guaranteeing that
//! causally-later events compare greater, which is what last-writer-wins needs.
//!
//! Reference: Kulkarni et al., "Logical Physical Clocks" (2014).

use std::cmp::Ordering;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::id::DeviceId;

/// A remote timestamp further ahead than this is treated as a misconfigured
/// clock: we still accept the event (dropping a clip is worse than a skewed
/// timestamp) but we refuse to drag our own clock along with it.
pub const MAX_CLOCK_DRIFT_MS: u64 = 60_000;

/// A point in the hybrid logical time of one device.
///
/// Ordering is `(wall_ms, counter, device)`. The device id is only a
/// tie-breaker; it makes the order total so two devices independently merging
/// the same pair of clips always pick the same winner.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Hlc {
    pub wall_ms: u64,
    pub counter: u32,
    pub device: DeviceId,
}

impl Hlc {
    pub fn new(wall_ms: u64, counter: u32, device: DeviceId) -> Self {
        Self {
            wall_ms,
            counter,
            device,
        }
    }
}

impl Ord for Hlc {
    fn cmp(&self, other: &Self) -> Ordering {
        self.wall_ms
            .cmp(&other.wall_ms)
            .then(self.counter.cmp(&other.counter))
            .then(self.device.as_uuid().cmp(&other.device.as_uuid()))
    }
}

impl PartialOrd for Hlc {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The clock itself. Cheap to clone via `Arc` at the call site; internally a
/// mutex because `now()` mutates and is called from both the clipboard watcher
/// thread and the sync task.
#[derive(Debug)]
pub struct HlcClock {
    device: DeviceId,
    state: Mutex<State>,
    /// Injectable for tests; production uses the system clock.
    now_ms: fn() -> u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct State {
    wall_ms: u64,
    counter: u32,
}

impl HlcClock {
    pub fn new(device: DeviceId) -> Self {
        Self {
            device,
            state: Mutex::new(State::default()),
            now_ms: system_now_ms,
        }
    }

    #[cfg(test)]
    fn with_clock(device: DeviceId, now_ms: fn() -> u64) -> Self {
        Self {
            device,
            state: Mutex::new(State::default()),
            now_ms,
        }
    }

    pub fn device(&self) -> DeviceId {
        self.device
    }

    /// Timestamp a locally-originated event. Strictly increasing, even if the
    /// wall clock stalls or jumps backwards.
    pub fn now(&self) -> Hlc {
        let physical = (self.now_ms)();
        let mut state = self.state.lock().expect("hlc mutex poisoned");

        if physical > state.wall_ms {
            state.wall_ms = physical;
            state.counter = 0;
        } else {
            // Wall clock did not advance (same millisecond, or it went
            // backwards) — the counter carries the ordering instead.
            state.counter = state.counter.saturating_add(1);
        }

        Hlc::new(state.wall_ms, state.counter, self.device)
    }

    /// Merge a timestamp received from a peer and return our new local time.
    ///
    /// After this returns, any event we stamp is ordered strictly after
    /// `remote`, which is what makes causality hold across devices.
    pub fn observe(&self, remote: &Hlc) -> Hlc {
        let physical = (self.now_ms)();
        let mut state = self.state.lock().expect("hlc mutex poisoned");

        // A peer far in the future must not pull our clock with it, or one
        // misconfigured machine would poison every device it ever syncs with.
        let remote_wall = if remote.wall_ms > physical.saturating_add(MAX_CLOCK_DRIFT_MS) {
            physical
        } else {
            remote.wall_ms
        };

        let max_wall = state.wall_ms.max(remote_wall).max(physical);

        state.counter = match (max_wall == state.wall_ms, max_wall == remote_wall) {
            (true, true) => state.counter.max(remote.counter).saturating_add(1),
            (true, false) => state.counter.saturating_add(1),
            (false, true) => remote.counter.saturating_add(1),
            (false, false) => 0,
        };
        state.wall_ms = max_wall;

        Hlc::new(state.wall_ms, state.counter, self.device)
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
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    use super::*;

    static FAKE_NOW: AtomicU64 = AtomicU64::new(1_000);

    fn fake_now() -> u64 {
        FAKE_NOW.load(AtomicOrdering::SeqCst)
    }

    fn set_now(ms: u64) {
        FAKE_NOW.store(ms, AtomicOrdering::SeqCst);
    }

    fn clock() -> HlcClock {
        HlcClock::with_clock(DeviceId::generate(), fake_now)
    }

    #[test]
    fn now_is_strictly_increasing_within_one_millisecond() {
        set_now(1_000);
        let c = clock();
        let a = c.now();
        let b = c.now();
        let d = c.now();
        assert!(a < b && b < d, "{a:?} {b:?} {d:?}");
        assert_eq!((a.counter, b.counter, d.counter), (0, 1, 2));
        assert_eq!(a.wall_ms, 1_000);
    }

    #[test]
    fn wall_clock_advance_resets_counter() {
        set_now(1_000);
        let c = clock();
        c.now();
        c.now();
        set_now(2_000);
        let t = c.now();
        assert_eq!((t.wall_ms, t.counter), (2_000, 0));
    }

    #[test]
    fn backwards_wall_clock_still_increases() {
        set_now(5_000);
        let c = clock();
        let before = c.now();
        set_now(4_000); // NTP correction, suspend/resume, user changed the clock
        let after = c.now();
        assert!(after > before, "HLC must not go backwards");
        assert_eq!(after.wall_ms, 5_000);
    }

    #[test]
    fn observe_orders_after_remote() {
        set_now(1_000);
        let local = clock();
        let remote = Hlc::new(1_500, 7, DeviceId::generate());
        let merged = local.observe(&remote);
        assert!(merged > remote, "{merged:?} !> {remote:?}");
        assert_eq!(merged.wall_ms, 1_500);
        assert_eq!(merged.counter, 8);
        // And subsequent local events stay after the remote one.
        assert!(local.now() > merged);
    }

    #[test]
    fn observe_ignores_stale_remote() {
        set_now(9_000);
        let local = clock();
        let mine = local.now();
        let stale = Hlc::new(1_000, 0, DeviceId::generate());
        let merged = local.observe(&stale);
        assert!(merged > mine);
        assert_eq!(merged.wall_ms, 9_000);
    }

    #[test]
    fn observe_clamps_a_peer_from_the_far_future() {
        set_now(10_000);
        let local = clock();
        let future = Hlc::new(10_000 + MAX_CLOCK_DRIFT_MS * 10, 0, DeviceId::generate());
        let merged = local.observe(&future);
        assert!(
            merged.wall_ms <= 10_000 + MAX_CLOCK_DRIFT_MS,
            "a broken peer clock poisoned ours: {merged:?}"
        );
    }

    #[test]
    fn device_id_breaks_ties_deterministically() {
        let a = DeviceId::generate();
        let b = DeviceId::generate();
        let (lo, hi) = if a.as_uuid() < b.as_uuid() {
            (a, b)
        } else {
            (b, a)
        };
        assert!(Hlc::new(1, 0, lo) < Hlc::new(1, 0, hi));
    }

    #[test]
    fn concurrent_devices_converge_on_the_same_winner() {
        // Two devices stamp independently; both must agree which is "later".
        set_now(1_000);
        let d1 = clock();
        let d2 = clock();
        let t1 = d1.now();
        let t2 = d2.now();
        let winner_from_1 = if t1 > t2 { t1 } else { t2 };
        let winner_from_2 = if t2 > t1 { t2 } else { t1 };
        assert_eq!(winner_from_1, winner_from_2);
    }
}
