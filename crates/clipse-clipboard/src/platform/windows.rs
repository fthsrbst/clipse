//! Windows backend: `AddClipboardFormatListener` + `WM_CLIPBOARDUPDATE` on a
//! hidden message-only window (`HWND_MESSAGE`). Event-driven — there is no
//! polling loop, unlike the macOS backend, because Windows tells us exactly
//! when the clipboard sequence number changes.
//!
//! A window's message queue is only pumped by the thread that created it, so
//! the whole lifecycle (create window, add the listener, run `GetMessageW`,
//! tear down) happens on one dedicated OS thread. All the OS-facing logic in
//! this file runs there; `WindowsClipboard::write` is the one exception,
//! since `OpenClipboard`/`SetClipboardData` are documented as callable from
//! any thread.

use std::mem::size_of;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use clipse_core::ClipFormat;
use tokio::sync::mpsc;
use windows::Win32::Foundation::{
    CloseHandle, GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardOwner, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    RemoveClipboardFormatListener, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetWindowThreadProcessId, HWND_MESSAGE, MSG, PostMessageW, RegisterClassExW, TranslateMessage,
    WM_APP, WM_CLIPBOARDUPDATE, WNDCLASSEXW,
};
use windows::core::{Error as WinError, PCWSTR, PWSTR, w};

use crate::capture::{Capture, Clipboard, hash_payloads};
use crate::error::{Error, Result};
use crate::own_write_guard::OwnWriteGuard;
use crate::watch::{CaptureEvent, RawPoll, WatchConfig, WatchMode, Watcher, classify, deliver};

/// Posted to our own window to ask the message loop to shut down. `WM_APP`
/// and above is reserved for application-defined messages.
const WM_APP_STOP: u32 = WM_APP + 1;

/// `HWND` wraps a raw pointer, so windows-rs does not implement `Send`/`Sync`
/// for it. It is safe to hand the value itself to another thread here: it is
/// never dereferenced, only passed back into Win32 calls that are documented
/// as safe to invoke from any thread (`PostMessageW`, `OpenClipboard`,
/// `SetClipboardData`, ...).
#[derive(Clone, Copy)]
struct SendHwnd(HWND);
unsafe impl Send for SendHwnd {}
unsafe impl Sync for SendHwnd {}

pub(crate) fn start(
    config: WatchConfig,
    guard: Arc<OwnWriteGuard>,
    tx: mpsc::Sender<CaptureEvent>,
) -> Result<Watcher> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<SendHwnd>>();
    let thread_guard = Arc::clone(&guard);
    let stopping = Arc::new(AtomicBool::new(false));
    let thread_stopping = Arc::clone(&stopping);

    let join = thread::Builder::new()
        .name("clipse-clipboard-watch".into())
        .spawn(move || message_loop(config, thread_guard, tx, ready_tx, thread_stopping))
        .map_err(|e| Error::WatcherStartup(format!("failed to spawn watch thread: {e}")))?;

    let hwnd = match ready_rx.recv() {
        Ok(Ok(hwnd)) => hwnd,
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(Error::WatcherStartup(
                "watch thread exited before it finished starting up".into(),
            ));
        }
    };

    let backend: Arc<dyn Clipboard> = Arc::new(WindowsClipboard { owner: hwnd, guard });
    let mut join = Some(join);
    let stopper: Box<dyn FnOnce() + Send + Sync> = Box::new(move || {
        // Referencing the whole `hwnd` (rather than only the `.0` field used
        // below) opts out of Rust 2021's disjoint closure capture, which
        // would otherwise capture just the inner `HWND`/`*mut c_void` field
        // and lose `SendHwnd`'s `unsafe impl Send`.
        let _ = &hwnd;
        // Set before posting: the watch thread may be parked waiting for room
        // on a full event channel, and this is what lets it give up on that
        // send instead of waiting for a consumer that is about to disappear.
        stopping.store(true, Ordering::SeqCst);
        // Best-effort: if the window is already gone the post fails and
        // there is nothing left to stop.
        unsafe {
            let _ = PostMessageW(Some(hwnd.0), WM_APP_STOP, WPARAM(0), LPARAM(0));
        }
        if let Some(handle) = join.take() {
            let _ = handle.join();
        }
    });

    Ok(Watcher::new(WatchMode::Automatic, backend, stopper))
}

/// Owns the window, the clipboard-format listener registration and the
/// message pump. Everything below runs on the dedicated thread spawned by
/// `start`, except where noted.
fn message_loop(
    config: WatchConfig,
    guard: Arc<OwnWriteGuard>,
    tx: mpsc::Sender<CaptureEvent>,
    ready: std::sync::mpsc::Sender<Result<SendHwnd>>,
    stopping: Arc<AtomicBool>,
) {
    let hwnd = match create_message_window() {
        Ok(hwnd) => hwnd,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };

    if let Err(e) = unsafe { AddClipboardFormatListener(hwnd.0) } {
        let _ = ready.send(Err(Error::Platform(format!(
            "AddClipboardFormatListener failed: {e}"
        ))));
        unsafe {
            let _ = DestroyWindow(hwnd.0);
        }
        return;
    }

    if ready.send(Ok(hwnd)).is_err() {
        // The constructor already gave up (e.g. it timed out or its own
        // thread panicked) — nothing left to serve, so tear down and exit
        // rather than pumping messages for a watcher nobody is holding.
        unsafe {
            let _ = RemoveClipboardFormatListener(hwnd.0);
            let _ = DestroyWindow(hwnd.0);
        }
        return;
    }

    loop {
        let mut msg = MSG::default();
        // `None` for the window filter: we still need to receive the
        // WM_APP_STOP message posted from another thread, which arrives
        // through the same queue as WM_CLIPBOARDUPDATE.
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if got.0 <= 0 {
            break; // 0 == WM_QUIT was posted; negative == GetMessageW error
        }

        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        match msg.message {
            WM_CLIPBOARDUPDATE => {
                let outcome = poll_with_retry(Some(hwnd.0));
                let event = classify(outcome, &config, &guard);
                if !deliver(&tx, event, &stopping) {
                    break; // receiver dropped, or we are shutting down
                }
            }
            // Teardown happens here rather than in a WM_DESTROY arm: Windows
            // *sends* WM_DESTROY straight to the window procedure instead of
            // posting it, so it never comes back out of GetMessageW and a
            // WM_DESTROY arm in this loop would never run — leaving the thread
            // parked in GetMessageW forever and `drop(watcher)` hung in join.
            WM_APP_STOP => {
                unsafe {
                    let _ = RemoveClipboardFormatListener(hwnd.0);
                    let _ = DestroyWindow(hwnd.0);
                }
                break;
            }
            _ => {}
        }
    }

    // The window class is intentionally left registered: its name is unique
    // per watcher instance (see `create_message_window`), so it cannot
    // collide with a later watcher, and Windows reclaims every class a
    // process registered when that process exits. Not worth the extra
    // Win32 call on every stop for a resource this cheap and this bounded.
    let _ = hwnd;
}

fn create_message_window() -> Result<SendHwnd> {
    static CLASS_COUNTER: AtomicU64 = AtomicU64::new(0);

    unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null())
            .map_err(|e| Error::Platform(format!("GetModuleHandleW failed: {e}")))?
            .into();

        // A unique class name per watcher instance, not a fixed constant:
        // `RegisterClassExW` fails with "class already exists" if two
        // watchers (e.g. two tests in one process) tried to share one name.
        let id = CLASS_COUNTER.fetch_add(1, Ordering::Relaxed);
        let class_name_wide: Vec<u16> = format!("ClipseClipboardWatcher-{id}\0")
            .encode_utf16()
            .collect();
        let class_name = PCWSTR(class_name_wide.as_ptr());

        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            lpszClassName: class_name,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err(Error::Platform(format!(
                "RegisterClassExW failed: {}",
                WinError::from_thread()
            )));
        }

        let hwnd = CreateWindowExW(
            Default::default(),
            class_name,
            PCWSTR::null(),
            Default::default(),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(HWND_MESSAGE),
            None,
            Some(hinstance),
            None,
        )
        .map_err(|e| Error::Platform(format!("CreateWindowExW failed: {e}")))?;

        Ok(SendHwnd(hwnd))
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // All real handling happens in `message_loop`, which inspects `MSG` after
    // `GetMessageW` returns; this exists only because window creation requires
    // a real function pointer and synchronously dispatches a few messages
    // (WM_NCCREATE, WM_CREATE) before the loop ever starts.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// `OpenClipboard` legitimately fails when another process is mid-copy; a
/// short retry is standard practice (Windows itself documents the clipboard
/// as briefly contended) rather than dropping a real capture.
fn poll_with_retry(owner: Option<HWND>) -> RawPoll {
    const ATTEMPTS: u32 = 5;
    for attempt in 0..ATTEMPTS {
        match with_clipboard_open(owner, poll_clipboard) {
            Ok(outcome) => return outcome,
            Err(_) if attempt + 1 < ATTEMPTS => thread::sleep(Duration::from_millis(15)),
            Err(_) => break,
        }
    }
    RawPoll::Empty
}

struct OpenClipboardGuard;
impl Drop for OpenClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn with_clipboard_open<T>(owner: Option<HWND>, f: impl FnOnce() -> Result<T>) -> Result<T> {
    unsafe { OpenClipboard(owner) }
        .map_err(|e| Error::Platform(format!("OpenClipboard failed: {e}")))?;
    let _guard = OpenClipboardGuard;
    f()
}

struct Formats {
    html: u32,
    rtf: u32,
    png: u32,
    exclude: u32,
}

/// Registered once per process — `RegisterClipboardFormatW` is idempotent
/// (repeat calls with the same name return the same id) but there is no
/// reason to pay the round trip on every poll.
fn formats() -> &'static Formats {
    static FORMATS: OnceLock<Formats> = OnceLock::new();
    FORMATS.get_or_init(|| unsafe {
        Formats {
            html: RegisterClipboardFormatW(w!("HTML Format")),
            rtf: RegisterClipboardFormatW(w!("Rich Text Format")),
            png: RegisterClipboardFormatW(w!("PNG")),
            exclude: RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")),
        }
    })
}

fn poll_clipboard() -> Result<RawPoll> {
    let fmts = formats();

    // Password managers that set this format are asking every clipboard
    // monitor, not just Clipse, to leave their content alone. We honour that
    // before reading anything else on the clipboard.
    if unsafe { IsClipboardFormatAvailable(fmts.exclude) }.is_ok() {
        return Ok(RawPoll::Concealed);
    }

    let mut payloads = Vec::new();

    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32) }.is_ok()
        && let Some(bytes) = read_unicode_text()?
    {
        payloads.push((ClipFormat::Text, bytes));
    }
    if unsafe { IsClipboardFormatAvailable(fmts.html) }.is_ok()
        && let Some(bytes) = read_registered_format(fmts.html)?
    {
        payloads.push((ClipFormat::Html, parse_cf_html(&bytes)));
    }
    if unsafe { IsClipboardFormatAvailable(fmts.rtf) }.is_ok()
        && let Some(bytes) = read_registered_format(fmts.rtf)?
    {
        payloads.push((ClipFormat::Rtf, strip_trailing_nuls(bytes)));
    }
    if unsafe { IsClipboardFormatAvailable(fmts.png) }.is_ok()
        && let Some(bytes) = read_registered_format(fmts.png)?
    {
        payloads.push((ClipFormat::Png, bytes));
    }
    if unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32) }.is_ok()
        && let Some(list) = read_hdrop()?
    {
        payloads.push((ClipFormat::FileList, list));
    }

    if payloads.is_empty() {
        return Ok(RawPoll::Empty);
    }

    Ok(RawPoll::Data(Capture {
        payloads,
        app: owning_app(),
    }))
}

fn read_global_bytes(handle: HANDLE) -> Result<Vec<u8>> {
    let hglobal = HGLOBAL(handle.0);
    let size = unsafe { GlobalSize(hglobal) };
    if size == 0 {
        return Ok(Vec::new());
    }
    let ptr = unsafe { GlobalLock(hglobal) };
    if ptr.is_null() {
        return Err(Error::Platform(
            "GlobalLock returned null while reading".into(),
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, size) }.to_vec();
    // GlobalUnlock's return value is not a reliable success/failure signal:
    // Win32 defines it to report the *new* lock count, so unlocking down to
    // zero (the expected, successful case for our single GlobalLock call)
    // reads as BOOL(FALSE) — which windows-rs maps to `Err`. Intentionally
    // ignored.
    unsafe {
        let _ = GlobalUnlock(hglobal);
    }
    Ok(bytes)
}

fn read_unicode_text() -> Result<Option<Vec<u8>>> {
    let Ok(handle) = (unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32) }) else {
        return Ok(None);
    };
    let raw = read_global_bytes(handle)?;
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    Ok(Some(String::from_utf16_lossy(&units[..end]).into_bytes()))
}

fn read_registered_format(format: u32) -> Result<Option<Vec<u8>>> {
    let Ok(handle) = (unsafe { GetClipboardData(format) }) else {
        return Ok(None);
    };
    Ok(Some(read_global_bytes(handle)?))
}

fn strip_trailing_nuls(mut bytes: Vec<u8>) -> Vec<u8> {
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    bytes
}

/// Layout of the Win32 `DROPFILES` header preceding a `CF_HDROP` payload:
/// a `u32` byte-offset to the file list, a `POINT`, and two `BOOL`s. Defined
/// locally (rather than pulled from `windows::Win32::UI::Shell`, which would
/// need another crate feature) since we only ever read/write these four
/// fixed-size fields.
#[repr(C)]
struct DropFilesHeader {
    p_files: u32,
    pt_x: i32,
    pt_y: i32,
    f_nc: i32,
    f_wide: i32,
}

fn read_hdrop() -> Result<Option<Vec<u8>>> {
    let Ok(handle) = (unsafe { GetClipboardData(CF_HDROP.0 as u32) }) else {
        return Ok(None);
    };
    let raw = read_global_bytes(handle)?;
    if raw.len() < size_of::<DropFilesHeader>() {
        return Ok(None);
    }
    let header: DropFilesHeader = unsafe { std::ptr::read_unaligned(raw.as_ptr().cast()) };
    let offset = header.p_files as usize;
    if offset > raw.len() {
        return Ok(None);
    }

    let list = &raw[offset..];
    let paths = if header.f_wide != 0 {
        let units: Vec<u16> = list
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        split_double_null_utf16(&units)
    } else {
        // Legacy ANSI drop lists are vanishingly rare on modern Windows;
        // decoded as Latin-1/lossy UTF-8 best-effort rather than pulling in
        // codepage-conversion machinery for a path we do not expect to hit.
        list.split(|&b| b == 0)
            .take_while(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    };

    if paths.is_empty() {
        Ok(None)
    } else {
        Ok(Some(paths.join("\n").into_bytes()))
    }
}

fn split_double_null_utf16(units: &[u16]) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < units.len() {
        let Some(rel) = units[start..].iter().position(|&c| c == 0) else {
            break;
        };
        if rel == 0 {
            break; // empty segment marks the list's double-null terminator
        }
        out.push(String::from_utf16_lossy(&units[start..start + rel]));
        start += rel + 1;
    }
    out
}

/// Best-effort: several platforms (this one included, for windows created by
/// elevated or protected processes) cannot always name the owner, so `None`
/// is an expected, non-error outcome.
fn owning_app() -> Option<String> {
    let owner = unsafe { GetClipboardOwner() }.ok()?;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(owner, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }

    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let _guard = HandleGuard(process);

    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    }
    .ok()?;

    let path = String::from_utf16_lossy(&buf[..len as usize]);
    Path::new(&path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
}

struct HandleGuard(HANDLE);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Every representation Clipse writes back is at most one Win32 clipboard
/// format; `Jpeg`/`Svg`/`Other` are not wired to one and are silently skipped
/// on write (a partial clipboard write is better than failing the whole
/// operation over one representation another app is unlikely to have asked
/// for anyway) — see the crate-level report for why.
struct WindowsClipboard {
    owner: SendHwnd,
    guard: Arc<OwnWriteGuard>,
}

impl Clipboard for WindowsClipboard {
    fn read(&self) -> Result<Option<Capture>> {
        // `None`, not our watch window: `OpenClipboard` associates the
        // clipboard with the *calling* thread, and `read` is called from
        // whichever thread the daemon happens to be on — not the one that
        // owns the message window. Reading needs no ownership anyway; only
        // `EmptyClipboard`/`SetClipboardData` care who the owner is.
        //
        // Retried for the same reason the watch loop retries: another process
        // mid-copy legitimately holds the clipboard for a moment.
        match poll_with_retry(None) {
            RawPoll::Data(capture) => Ok(Some(capture)),
            RawPoll::Concealed | RawPoll::Empty => Ok(None),
        }
    }

    fn write(&self, payloads: &[(ClipFormat, Vec<u8>)]) -> Result<()> {
        with_clipboard_open(Some(self.owner.0), || unsafe {
            EmptyClipboard().map_err(|e| Error::Platform(format!("EmptyClipboard failed: {e}")))?;
            for (format, bytes) in payloads {
                write_one(format, bytes)?;
            }
            Ok(())
        })?;
        self.guard.record_write(hash_payloads(payloads));
        Ok(())
    }
}

unsafe fn write_one(format: &ClipFormat, bytes: &[u8]) -> Result<()> {
    match format {
        ClipFormat::Text => {
            let text = String::from_utf8_lossy(bytes);
            let mut units: Vec<u16> = text.encode_utf16().collect();
            units.push(0);
            let wire: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
            unsafe { set_global(CF_UNICODETEXT.0 as u32, &wire) }
        }
        ClipFormat::Html => unsafe { set_global(formats().html, &build_cf_html(bytes)) },
        ClipFormat::Rtf => {
            let mut wire = bytes.to_vec();
            wire.push(0);
            unsafe { set_global(formats().rtf, &wire) }
        }
        ClipFormat::Png => unsafe { set_global(formats().png, bytes) },
        ClipFormat::FileList => unsafe { set_global(CF_HDROP.0 as u32, &build_hdrop(bytes)) },
        ClipFormat::Jpeg | ClipFormat::Svg | ClipFormat::Other(_) => Ok(()),
    }
}

unsafe fn set_global(format: u32, bytes: &[u8]) -> Result<()> {
    let hglobal = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) }
        .map_err(|e| Error::Platform(format!("GlobalAlloc failed: {e}")))?;

    let ptr = unsafe { GlobalLock(hglobal) };
    if ptr.is_null() {
        unsafe {
            let _ = GlobalFree(Some(hglobal));
        }
        return Err(Error::Platform(
            "GlobalLock returned null while writing".into(),
        ));
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast(), bytes.len());
        let _ = GlobalUnlock(hglobal); // see read_global_bytes on why this is ignored
    }

    // `SetClipboardData` takes ownership of `hglobal` once it succeeds — we
    // must not free it ourselves in that case, only on failure.
    match unsafe { SetClipboardData(format, Some(HANDLE(hglobal.0))) } {
        Ok(_) => Ok(()),
        Err(e) => {
            unsafe {
                let _ = GlobalFree(Some(hglobal));
            }
            Err(Error::Platform(format!("SetClipboardData failed: {e}")))
        }
    }
}

/// Extracts the `<!--StartFragment-->`/`<!--EndFragment-->` slice out of a
/// `CF_HTML` byte buffer using the `StartFragment:`/`EndFragment:` byte
/// offsets in its header, so `ClipFormat::Html` payloads are the plain HTML
/// fragment on every platform (macOS/Linux HTML clipboard formats are
/// already just a fragment, with no such wrapper). Falls back to the raw
/// bytes if the header does not parse — better to hand back something than
/// nothing.
fn parse_cf_html(bytes: &[u8]) -> Vec<u8> {
    let header = String::from_utf8_lossy(bytes);
    match (
        find_offset(&header, "StartFragment:"),
        find_offset(&header, "EndFragment:"),
    ) {
        (Some(start), Some(end)) if start < end && end <= bytes.len() => bytes[start..end].to_vec(),
        _ => bytes.to_vec(),
    }
}

fn find_offset(header: &str, key: &str) -> Option<usize> {
    let after_key = &header[header.find(key)? + key.len()..];
    let digits: String = after_key.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Wraps a plain HTML fragment in the header Windows' `CF_HTML` format
/// requires. All four byte-offset fields are zero-padded to 9 digits, which
/// keeps the header's length fixed regardless of the (unpadded) offset
/// values, so the offsets can be computed in one pass instead of two.
fn build_cf_html(fragment: &[u8]) -> Vec<u8> {
    const PREFIX: &str = "<html>\r\n<body>\r\n<!--StartFragment-->";
    const SUFFIX: &str = "<!--EndFragment-->\r\n</body>\r\n</html>\r\n";

    let header_of =
        |start_html: usize, end_html: usize, start_fragment: usize, end_fragment: usize| {
            format!(
                "Version:0.9\r\nStartHTML:{start_html:09}\r\nEndHTML:{end_html:09}\r\n\
             StartFragment:{start_fragment:09}\r\nEndFragment:{end_fragment:09}\r\n"
            )
        };

    let header_len = header_of(0, 0, 0, 0).len();
    let start_html = header_len;
    let start_fragment = start_html + PREFIX.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + SUFFIX.len();

    let mut out = header_of(start_html, end_html, start_fragment, end_fragment).into_bytes();
    out.extend_from_slice(PREFIX.as_bytes());
    out.extend_from_slice(fragment);
    out.extend_from_slice(SUFFIX.as_bytes());
    out
}

/// Builds a `CF_HDROP` payload (a `DROPFILES` header followed by a
/// double-null-terminated, UTF-16 file list) from the newline-separated path
/// list `read_hdrop` produces — the round trip this crate controls both
/// ends of; other producers of `ClipFormat::FileList` are expected to follow
/// the same convention.
fn build_hdrop(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let mut units: Vec<u16> = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        units.extend(line.encode_utf16());
        units.push(0);
    }
    units.push(0); // second, list-terminating null

    let header_len = size_of::<DropFilesHeader>();
    let header = DropFilesHeader {
        p_files: header_len as u32,
        pt_x: 0,
        pt_y: 0,
        f_nc: 0,
        f_wide: 1,
    };

    let mut out = vec![0u8; header_len];
    unsafe { std::ptr::write_unaligned(out.as_mut_ptr().cast(), header) };
    out.extend(units.iter().flat_map(|u| u.to_le_bytes()));
    out
}
