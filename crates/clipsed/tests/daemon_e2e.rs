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
use clipse_ipc::protocol::{CaptureMode, Event, HistoryQuery, Request, Response};
use clipse_ipc::{Client, IPC_VERSION};
use tempfile::TempDir;

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
async fn daemon_starts_serves_status_and_persists_settings() {
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
    /// These tests take over the machine's one clipboard, so they must not run
    /// alongside each other — two interleaving would each see the other's
    /// copies. Held for the whole body of every clipboard test.
    fn clipboard_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

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
    // Guards against a client and daemon drifting apart silently.
    assert_eq!(IPC_VERSION, 1);
}
