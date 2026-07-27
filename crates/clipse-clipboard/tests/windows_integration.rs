//! End-to-end tests against the real Windows clipboard. Only compiled and run
//! on Windows — this is the one platform available in this environment, and
//! per the crate's constraints it is the only backend that can be verified
//! at all here.
//!
//! These tests take over the machine's actual clipboard for their duration.
//! That is an unavoidable consequence of testing a real clipboard
//! integration rather than a mock, and is the reason this lives in `tests/`
//! (run serially, once per process) instead of the crate's unit tests.
#![cfg(windows)]

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use clipse_clipboard::{CaptureEvent, Clipboard, SuppressionReason, WatchConfig, watch};
use clipse_core::ClipFormat;
use tokio::time::timeout;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// The machine has exactly one clipboard, and every watcher in this process
/// sees every change to it. Run in parallel, these tests capture each other's
/// copies and fail on whichever event happened to arrive first, so each one
/// takes this lock for its whole duration — start watcher, act, assert, drop.
///
/// `#[tokio::test]` builds a current-thread runtime, so holding a plain
/// `MutexGuard` across an await here is fine: the future is never required to
/// be `Send`, and each test is the only thing running on its runtime — the
/// deadlock `clippy::await_holding_lock` warns about needs a second task on
/// the same executor wanting the same lock, which cannot happen here.
#[allow(clippy::await_holding_lock)]
fn clipboard_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Simulates a real, external copy (as if the user pressed Ctrl+C in some
/// other application) — deliberately *not* going through this crate's
/// `Clipboard::write`, so the resulting capture must NOT be suppressed as
/// `OwnWrite`.
fn simulate_external_copy(text: &str) {
    unsafe {
        OpenClipboard(None).expect("OpenClipboard");
        EmptyClipboard().expect("EmptyClipboard");

        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.push(0);
        let bytes_len = units.len() * 2;

        let hglobal = GlobalAlloc(GMEM_MOVEABLE, bytes_len).expect("GlobalAlloc");
        let ptr = GlobalLock(hglobal);
        assert!(!ptr.is_null(), "GlobalLock");
        std::ptr::copy_nonoverlapping(units.as_ptr().cast::<u8>(), ptr.cast(), bytes_len);
        let _ = GlobalUnlock(hglobal);

        SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hglobal.0)))
            .expect("SetClipboardData");
        CloseClipboard().expect("CloseClipboard");
    }
}

async fn next_event(rx: &mut tokio::sync::mpsc::Receiver<CaptureEvent>) -> CaptureEvent {
    timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for a capture event")
        .expect("watcher channel closed unexpectedly")
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // see clipboard_lock
async fn watch_reports_a_genuine_external_copy() {
    let _serial = clipboard_lock();

    let (watcher, mut rx) = watch(WatchConfig::default()).expect("watch() should start on Windows");

    let marker = format!("clipse-integration-test-{}", std::process::id());
    simulate_external_copy(&marker);

    let event = next_event(&mut rx).await;
    match event {
        CaptureEvent::Captured(capture) => {
            let text = capture
                .payloads
                .iter()
                .find(|(f, _)| *f == ClipFormat::Text)
                .map(|(_, b)| String::from_utf8_lossy(b).into_owned());
            assert_eq!(text.as_deref(), Some(marker.as_str()));
        }
        other => panic!("expected a Captured event for an external copy, got {other:?}"),
    }

    drop(watcher);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // see clipboard_lock
async fn write_through_the_watcher_suppresses_its_own_echo() {
    let _serial = clipboard_lock();

    let (watcher, mut rx) = watch(WatchConfig::default()).expect("watch() should start on Windows");

    let marker = format!("clipse-own-write-{}", std::process::id());
    watcher
        .write(&[(ClipFormat::Text, marker.clone().into_bytes())])
        .expect("write via the watcher should succeed");

    // The very next event must be our own echo, suppressed — not a fresh
    // capture of content we just wrote ourselves.
    let event = next_event(&mut rx).await;
    assert!(
        matches!(event, CaptureEvent::Suppressed(SuppressionReason::OwnWrite)),
        "expected the write's own echo to be suppressed, got {event:?}"
    );

    // A later, genuine external copy of the *same* text is a real user
    // action and must not be suppressed forever by that one guard entry.
    simulate_external_copy(&marker);
    let event = next_event(&mut rx).await;
    assert!(
        matches!(event, CaptureEvent::Captured(_)),
        "a later external copy of the same content must not be suppressed, got {event:?}"
    );

    drop(watcher);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // see clipboard_lock
async fn clipboard_read_and_write_round_trip_without_the_watch_loop() {
    let _serial = clipboard_lock();

    let (watcher, _rx) = watch(WatchConfig::default()).expect("watch() should start on Windows");

    let text = format!("clipse-roundtrip-{}", std::process::id());
    watcher
        .write(&[(ClipFormat::Text, text.clone().into_bytes())])
        .expect("write");

    let captured = watcher
        .read()
        .expect("read")
        .expect("clipboard should not be empty");
    let read_back = captured
        .payloads
        .iter()
        .find(|(f, _)| *f == ClipFormat::Text)
        .map(|(_, b)| String::from_utf8_lossy(b).into_owned());
    assert_eq!(read_back.as_deref(), Some(text.as_str()));

    drop(watcher);
}
