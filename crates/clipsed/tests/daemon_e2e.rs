//! End-to-end: spawn the real `clipsed` binary, copy something the way a user
//! would, and drive it over IPC exactly as the UI will.
//!
//! The clipboard half is Windows-only for now — it drives the OS clipboard
//! through PowerShell so that the copy genuinely comes from another process,
//! not from inside our own address space. The daemon-lifecycle half runs
//! everywhere.

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use clipse_core::Paths;
// `CaptureMode` and `HistoryQuery` are only used by the Windows-only
// `clipboard` module below, so they are imported there rather than here —
// otherwise every non-Windows build warns, and `-D warnings` turns that into a
// failure on two thirds of the matrix.
use clipse_ipc::protocol::{Event, Request, Response};
use clipse_ipc::{Client, IPC_VERSION};
use tempfile::TempDir;

/// Serialises everything that touches the machine's one clipboard.
///
/// Two kinds of test contend for it: the ones that *write* it, and the ones that
/// start a daemon and then assert on what the daemon saw. `cargo test` runs both
/// in parallel by default, so a writer's copy lands in an unrelated daemon's
/// history — which is exactly how `a fresh data dir starts empty` failed on CI
/// while passing on every developer machine.
///
/// This lives at file scope rather than inside the Windows-only `clipboard`
/// module because the tests on both sides of the conflict do not: the clipboard
/// writers are in that module, and the daemon that gets confused by them is not.
///
/// Poisoning is ignored on purpose. One panicking test should fail by itself
/// rather than cascade into every test that takes this lock after it.
fn clipboard_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Kills the daemon when the test ends, including on panic — otherwise a
/// failing assertion would leave a process holding the machine's clipboard
/// listener.
struct DaemonProcess(Child);

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn start_daemon(dir: &TempDir) -> (DaemonProcess, String) {
    let paths = Paths::with_root(dir.path());
    let endpoint = paths.ipc_endpoint();

    let child = Command::new(env!("CARGO_BIN_EXE_clipsed"))
        .arg("--data-dir")
        .arg(dir.path())
        .arg("--log")
        .arg("warn")
        .spawn()
        .expect("spawn clipsed");

    (DaemonProcess(child), endpoint)
}

/// The daemon binds its socket a moment after launch; retry rather than
/// sleeping a guessed amount.
async fn connect(endpoint: &str) -> Client {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match Client::connect(endpoint, "e2e-test").await {
            Ok(client) => return client,
            Err(e) if Instant::now() < deadline => {
                let _ = e;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => panic!("daemon never became reachable on {endpoint}: {e}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
// Held across awaits deliberately: the point is to exclude other tests for the
// whole body, and these are `#[tokio::test]` functions on separate runtimes
// rather than tasks competing for one, so nothing can deadlock on it.
#[allow(clippy::await_holding_lock)]
async fn daemon_starts_serves_status_and_persists_settings() {
    // Asserts the history is empty, so no clipboard writer may run alongside it.
    let _clipboard = clipboard_test_lock();
    let dir = TempDir::new().unwrap();
    let (_daemon, endpoint) = start_daemon(&dir).await;
    let mut client = connect(&endpoint).await;

    let device = match client.call(Request::Status).await.unwrap() {
        Response::Status(status) => {
            assert_eq!(status.clip_count, 0, "a fresh data dir starts empty");
            assert_eq!(status.daemon_version, env!("CARGO_PKG_VERSION"));
            status.device
        }
        other => panic!("unexpected: {other:?}"),
    };

    let mut settings = match client.call(Request::GetSettings).await.unwrap() {
        Response::Settings(settings) => *settings,
        other => panic!("unexpected: {other:?}"),
    };
    assert!(settings.detect_secrets, "secret detection must default on");

    settings.hotkey = "Alt+Space".into();
    assert!(matches!(
        client
            .call(Request::UpdateSettings(Box::new(settings)))
            .await
            .unwrap(),
        Response::Ok
    ));

    // A fresh connection sees the change, and the device identity is stable.
    let mut second = connect(&endpoint).await;
    match second.call(Request::GetSettings).await.unwrap() {
        Response::Settings(settings) => assert_eq!(settings.hotkey, "Alt+Space"),
        other => panic!("unexpected: {other:?}"),
    }
    match second.call(Request::Status).await.unwrap() {
        Response::Status(status) => assert_eq!(status.device, device),
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_daemon_on_the_same_data_dir_refuses_to_start() {
    let dir = TempDir::new().unwrap();
    let (_first, endpoint) = start_daemon(&dir).await;
    let _client = connect(&endpoint).await;

    let second = Command::new(env!("CARGO_BIN_EXE_clipsed"))
        .arg("--data-dir")
        .arg(dir.path())
        .output()
        .expect("spawn second clipsed");

    assert!(
        !second.status.success(),
        "two daemons sharing a data directory would fight over one database"
    );
}

#[cfg(windows)]
mod clipboard {
    use super::*;

    use clipse_ipc::protocol::{CaptureMode, HistoryQuery};
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
    };
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    /// The clipboard is one global resource that any process may hold open for
    /// a moment, so `OpenClipboard` legitimately fails under contention — and
    /// the daemon under test opens it on every change it is notified about.
    /// Real applications retry; so does this.
    ///
    /// Win32 directly rather than `Set-Clipboard`: PowerShell's cmdlet reports
    /// failures it did not have and swallows ones it did, which made this test
    /// lie in both directions.
    fn with_clipboard<T>(what: &str, f: impl Fn() -> T) -> T {
        for _ in 0..40 {
            if unsafe { OpenClipboard(None) }.is_ok() {
                let value = f();
                unsafe {
                    let _ = CloseClipboard();
                }
                return value;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("{what}: could not open the clipboard");
    }

    /// Writes from the *test* process, which is a different process from the
    /// daemon — so from the daemon's point of view this is an ordinary
    /// external copy, exactly like a user pressing Ctrl+C in an editor.
    fn external_copy(text: &str) {
        with_clipboard("copy", || unsafe {
            EmptyClipboard().expect("EmptyClipboard");

            let mut units: Vec<u16> = text.encode_utf16().collect();
            units.push(0);
            let byte_len = units.len() * 2;

            let hglobal = GlobalAlloc(GMEM_MOVEABLE, byte_len).expect("GlobalAlloc");
            let ptr = GlobalLock(hglobal);
            assert!(!ptr.is_null(), "GlobalLock");
            std::ptr::copy_nonoverlapping(units.as_ptr().cast::<u8>(), ptr.cast(), byte_len);
            let _ = GlobalUnlock(hglobal);

            SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hglobal.0)))
                .expect("SetClipboardData");
        });
    }

    fn read_clipboard() -> String {
        with_clipboard("read", || unsafe {
            let Ok(handle) = GetClipboardData(CF_UNICODETEXT.0 as u32) else {
                return String::new();
            };
            let hglobal = HGLOBAL(handle.0);
            let size = GlobalSize(hglobal);
            let ptr = GlobalLock(hglobal);
            if ptr.is_null() {
                return String::new();
            }
            let bytes = std::slice::from_raw_parts(ptr as *const u8, size).to_vec();
            let _ = GlobalUnlock(hglobal);

            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
            String::from_utf16_lossy(&units[..end])
        })
    }

    async fn wait_for_clip(client: &mut Client, needle: &str) -> clipse_core::Clip {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let response = client
                .call(Request::History(HistoryQuery::page(50)))
                .await
                .unwrap();
            if let Response::Clips(clips) = response
                && let Some(found) = clips.into_iter().find(|c| c.preview.contains(needle))
            {
                return found;
            }
            assert!(
                Instant::now() < deadline,
                "clip containing {needle:?} never reached the history"
            );
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::await_holding_lock)] // see clipboard_test_lock
    async fn a_real_copy_reaches_the_history_and_can_be_applied_back() {
        let _serial = clipboard_test_lock();

        let dir = TempDir::new().unwrap();
        let (_daemon, endpoint) = start_daemon(&dir).await;
        let mut client = connect(&endpoint).await;

        match client.call(Request::Status).await.unwrap() {
            Response::Status(status) => assert_eq!(
                status.capture_mode,
                CaptureMode::Automatic,
                "Windows must capture automatically"
            ),
            other => panic!("unexpected: {other:?}"),
        }

        let marker = format!("clipse-e2e-{}", std::process::id());
        external_copy(&marker);

        let clip = wait_for_clip(&mut client, &marker).await;
        assert_eq!(clip.text(), Some(marker.as_str()));
        assert!(
            !clip.source.device_label.is_empty(),
            "source device recorded"
        );

        // Copy something else, then ask the daemon to put the first clip back.
        external_copy("something else entirely");
        wait_for_clip(&mut client, "something else").await;

        assert!(matches!(
            client.call(Request::Apply { id: clip.id }).await.unwrap(),
            Response::Ok
        ));
        assert_eq!(read_clipboard(), marker, "Apply did not restore the clip");

        // The daemon's own write must not come back as a new capture: that is
        // the loop guard, and without it every applied clip would duplicate.
        tokio::time::sleep(Duration::from_millis(600)).await;
        let clips = match client
            .call(Request::History(HistoryQuery::page(50)))
            .await
            .unwrap()
        {
            Response::Clips(clips) => clips,
            other => panic!("unexpected: {other:?}"),
        };
        let matching = clips
            .iter()
            .filter(|c| c.text() == Some(marker.as_str()))
            .count();
        assert_eq!(matching, 1, "applying a clip echoed back into the history");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::await_holding_lock)] // see clipboard_test_lock
    async fn a_detected_secret_never_reaches_the_history() {
        let _serial = clipboard_test_lock();

        let dir = TempDir::new().unwrap();
        let (_daemon, endpoint) = start_daemon(&dir).await;
        let mut client = connect(&endpoint).await;
        let mut events = connect(&endpoint).await.subscribe().await.unwrap();

        // A real AWS key shape. This must be suppressed at capture, so it can
        // reach neither the store nor (in F2) another device.
        external_copy("AKIAIOSFODNN7EXAMPLE");

        let suppressed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Event::Suppressed { reason } = events.next().await.unwrap() {
                    return reason;
                }
            }
        })
        .await
        .expect("no suppression event arrived");
        assert!(
            !suppressed.contains("AKIA"),
            "the suppression reason leaked the secret: {suppressed}"
        );

        // A refusal has to *push* the new count, not just hold it. The window
        // rereads `DaemonStatus` on `StatusChanged` and nowhere else, so a
        // suppression that emits only `Suppressed` leaves the spine reading
        // "0 refused" while it is refusing — which is exactly what an
        // installed copy did on macOS before this.
        //
        // It waits for a status *carrying the count* rather than for the next
        // one to arrive: peers and capture mode push status too, and a race
        // with one of those would fail this for the wrong reason.
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Event::StatusChanged(status) = events.next().await.unwrap()
                    && status.secrets_refused == 1
                {
                    return;
                }
            }
        })
        .await
        .expect("the suppression pushed no status update carrying the new count");

        // The count is the only record a refusal leaves, and it is what the
        // window shows. Asserted here rather than in a unit test because the
        // thing worth proving is that a *real* suppression reaches it — and
        // before the marker copy below, while the history is still empty.
        match client.call(Request::Status).await.unwrap() {
            Response::Status(status) => {
                assert_eq!(status.secrets_refused, 1);
                assert_eq!(
                    status.clip_count, 0,
                    "nothing about the refused copy reached the store"
                );
            }
            other => panic!("unexpected: {other:?}"),
        }

        // Copy something harmless afterwards so we know the daemon kept
        // running and the history simply does not contain the key.
        let marker = format!("clipse-after-secret-{}", std::process::id());
        external_copy(&marker);
        wait_for_clip(&mut client, &marker).await;

        match client
            .call(Request::History(HistoryQuery::page(50)))
            .await
            .unwrap()
        {
            Response::Clips(clips) => {
                assert!(
                    !clips.iter().any(|c| c.preview.contains("AKIA")),
                    "a detected secret was written to the history"
                );
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::await_holding_lock)] // see clipboard_test_lock
    async fn history_survives_a_daemon_restart() {
        let _serial = clipboard_test_lock();

        let dir = TempDir::new().unwrap();
        let marker = format!("clipse-restart-{}", std::process::id());

        {
            let (_daemon, endpoint) = start_daemon(&dir).await;
            let mut client = connect(&endpoint).await;
            external_copy(&marker);
            wait_for_clip(&mut client, &marker).await;
        } // daemon killed here

        let (_daemon, endpoint) = start_daemon(&dir).await;
        let mut client = connect(&endpoint).await;
        match client
            .call(Request::History(HistoryQuery::page(50)))
            .await
            .unwrap()
        {
            Response::Clips(clips) => assert!(
                clips.iter().any(|c| c.text() == Some(marker.as_str())),
                "history did not survive a restart"
            ),
            other => panic!("unexpected: {other:?}"),
        }
    }
}

#[test]
fn ipc_version_is_the_one_the_daemon_was_built_against() {
    // Guards against a client and daemon drifting apart silently. Changing
    // this number is the point at which you confirm the wire really did change
    // shape -- 3 replaced the pasted URI with a typed six-digit code.
    assert_eq!(IPC_VERSION, 3);
}

/// Only one pairing test at a time.
///
/// Each of these starts real daemons that announce themselves and then walk
/// *every* Clipse on this machine looking for the one showing a code — so two
/// running at once means each one's wrong-code probes land on the other's
/// offer, and an offer that is probed with enough wrong codes retires itself
/// (which is the point of that rule). Serialising them keeps the tests honest
/// about the behaviour instead of weakening it to make them pass.
static ONE_PAIRING_AT_A_TIME: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Two real daemon processes, paired the way the UI does it.
///
/// The whole ceremony over the real IPC surface and the real network: one
/// device shows six digits, the other is told those six digits and finds it,
/// and both prove to each other that they mean it. There is no third step —
/// the user types once and is done.
///
/// Finding the other device is part of what is under test: nothing here tells
/// Bob where Alice is.
#[tokio::test(flavor = "multi_thread")]
async fn two_daemons_pair_from_six_typed_digits() {
    let _one_at_a_time = ONE_PAIRING_AT_A_TIME.lock().await;
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    let (_daemon_a, endpoint_a) = start_daemon(&dir_a).await;
    let (_daemon_b, endpoint_b) = start_daemon(&dir_b).await;

    let mut alice = connect(&endpoint_a).await;
    let mut bob = connect(&endpoint_b).await;

    // Alice's second connection watches for the news that someone paired —
    // the offering device has no other way to know the ceremony finished.
    let mut alice_events = connect(&endpoint_a).await.subscribe().await.unwrap();

    let code = match alice.call(Request::BeginPairing).await.unwrap() {
        Response::PairingCode {
            code,
            expires_at_ms,
        } => {
            assert_eq!(
                code.chars().filter(char::is_ascii_digit).count(),
                6,
                "the screen must show six digits, not {code}"
            );
            assert!(expires_at_ms > 0, "a code must expire");
            code
        }
        other => panic!("BeginPairing: {other:?}"),
    };

    // Nothing is trusted yet, on either side.
    for client in [&mut alice, &mut bob] {
        match client.call(Request::Status).await.unwrap() {
            Response::Status(status) => assert_eq!(
                status.peers_total, 0,
                "a device was trusted before anyone typed anything"
            ),
            other => panic!("Status: {other:?}"),
        }
    }

    match bob
        .call(Request::PairWithCode { code })
        .await
        .expect("typing the code must pair")
    {
        Response::Paired { peer_label } => {
            assert!(!peer_label.is_empty(), "the other device should be named")
        }
        other => panic!("PairWithCode: {other:?}"),
    }

    let told = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Event::PairingSucceeded { peer_label } = alice_events.next().await.unwrap() {
                return peer_label;
            }
        }
    })
    .await
    .expect("the offering device was never told it had paired");
    assert!(!told.is_empty());

    for client in [&mut alice, &mut bob] {
        match client.call(Request::Status).await.unwrap() {
            Response::Status(status) => {
                assert_eq!(status.peers_total, 1, "the ceremony did not pair them")
            }
            other => panic!("Status: {other:?}"),
        }
    }

    // And each device now lists the other, which is what the tray shows.
    match alice.call(Request::Devices).await.unwrap() {
        Response::Devices(devices) => {
            assert_eq!(devices.len(), 1, "the paired device must be listed");
            assert!(!devices[0].label.is_empty());
        }
        other => panic!("Devices: {other:?}"),
    }
}

/// A digit typed wrong must pair nothing at all — not a different device, not
/// the right device without the check.
#[tokio::test(flavor = "multi_thread")]
async fn a_mistyped_code_pairs_nothing() {
    let _one_at_a_time = ONE_PAIRING_AT_A_TIME.lock().await;
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    let (_daemon_a, endpoint_a) = start_daemon(&dir_a).await;
    let (_daemon_b, endpoint_b) = start_daemon(&dir_b).await;

    let mut alice = connect(&endpoint_a).await;
    let mut bob = connect(&endpoint_b).await;

    let code = match alice.call(Request::BeginPairing).await.unwrap() {
        Response::PairingCode { code, .. } => code,
        other => panic!("BeginPairing: {other:?}"),
    };

    let mistyped: String = code
        .chars()
        .map(|c| match c.to_digit(10) {
            Some(digit) => char::from_digit((digit + 1) % 10, 10).unwrap(),
            None => c,
        })
        .collect();

    assert!(
        bob.call(Request::PairWithCode { code: mistyped })
            .await
            .is_err(),
        "the wrong six digits must not pair anything"
    );

    for client in [&mut alice, &mut bob] {
        match client.call(Request::Status).await.unwrap() {
            Response::Status(status) => assert_eq!(
                status.peers_total, 0,
                "a device was trusted on a wrong code"
            ),
            other => panic!("Status: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn answering_a_device_that_is_not_offering_fails_cleanly() {
    let _one_at_a_time = ONE_PAIRING_AT_A_TIME.lock().await;
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    let (_daemon_a, endpoint_a) = start_daemon(&dir_a).await;
    let (_daemon_b, endpoint_b) = start_daemon(&dir_b).await;

    let mut alice = connect(&endpoint_a).await;
    let mut bob = connect(&endpoint_b).await;

    let code = match alice.call(Request::BeginPairing).await.unwrap() {
        Response::PairingCode { code, .. } => code,
        other => panic!("BeginPairing: {other:?}"),
    };

    // Alice closes the pairing screen before Bob gets round to typing.
    assert!(matches!(
        alice.call(Request::CancelPairing).await.unwrap(),
        Response::Ok
    ));

    assert!(
        bob.call(Request::PairWithCode { code }).await.is_err(),
        "a closed pairing window must refuse the code it was showing"
    );

    match bob.call(Request::Status).await.unwrap() {
        Response::Status(status) => assert_eq!(status.peers_total, 0),
        other => panic!("Status: {other:?}"),
    }
}
