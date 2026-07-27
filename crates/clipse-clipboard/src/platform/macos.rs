//! macOS backend: `NSPasteboard.changeCount` polled at 250 ms.
//!
//! There is no clipboard-change notification on macOS — no equivalent of
//! `WM_CLIPBOARDUPDATE` or XFIXES. `changeCount` is a monotonic counter that
//! AppKit bumps on every write, so polling it is the documented approach and
//! what every clipboard manager on the platform does. 250 ms is the interval
//! Apple's own sample code uses: fast enough that a copy-then-switch-window
//! feels instant, slow enough to stay invisible in Activity Monitor.
//!
//! `NSPasteboard` is not documented as thread-safe, and here it is touched
//! from both the poll thread and whichever thread calls `Clipboard::write`.
//! One process-wide mutex serialises every access rather than hoping the
//! races stay theoretical.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Duration;

use clipse_core::ClipFormat;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypePNG, NSPasteboardTypeRTF,
    NSPasteboardTypeString, NSPasteboardTypeTIFF,
};
use objc2_foundation::{NSCopying, NSData, NSString};
use tokio::sync::mpsc;

use crate::capture::{Capture, Clipboard, hash_payloads};
use crate::error::{Error, Result};
use crate::own_write_guard::OwnWriteGuard;
use crate::watch::{CaptureEvent, RawPoll, WatchConfig, WatchMode, Watcher, classify, deliver};

const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Cross-application convention (nspasteboard.org) that password managers and
/// other privacy-conscious apps use to tell clipboard managers to keep out.
/// Honoured before any payload is read.
const CONCEALED_TYPE: &str = "org.nspasteboard.ConcealedType";
/// Same convention, for content that is not secret but is not meant to be
/// remembered either (an app moving data between its own windows).
const TRANSIENT_TYPE: &str = "org.nspasteboard.TransientType";
/// A single file URL. Multiple files arrive as multiple pasteboard items.
const FILE_URL_TYPE: &str = "public.file-url";

fn pasteboard_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) fn start(
    config: WatchConfig,
    guard: Arc<OwnWriteGuard>,
    tx: mpsc::Sender<CaptureEvent>,
) -> Result<Watcher> {
    let stopping = Arc::new(AtomicBool::new(false));
    let thread_stopping = Arc::clone(&stopping);
    let thread_guard = Arc::clone(&guard);

    let join = thread::Builder::new()
        .name("clipse-clipboard-watch".into())
        .spawn(move || poll_loop(config, thread_guard, tx, thread_stopping))
        .map_err(|e| Error::WatcherStartup(format!("failed to spawn watch thread: {e}")))?;

    let backend: Arc<dyn Clipboard> = Arc::new(MacClipboard { guard });
    let mut join = Some(join);
    let stopper: Box<dyn FnOnce() + Send + Sync> = Box::new(move || {
        stopping.store(true, Ordering::SeqCst);
        if let Some(handle) = join.take() {
            let _ = handle.join();
        }
    });

    Ok(Watcher::new(WatchMode::Automatic, backend, stopper))
}

fn poll_loop(
    config: WatchConfig,
    guard: Arc<OwnWriteGuard>,
    tx: mpsc::Sender<CaptureEvent>,
    stopping: Arc<AtomicBool>,
) {
    // Seed from the current count so whatever happened to be on the clipboard
    // before Clipse started is not captured as if the user had just copied it.
    let mut last_change = current_change_count();

    while !stopping.load(Ordering::SeqCst) {
        thread::sleep(POLL_INTERVAL);
        if stopping.load(Ordering::SeqCst) {
            break;
        }

        let change = current_change_count();
        if change == last_change {
            continue;
        }
        last_change = change;

        let event = classify(poll_pasteboard(), &config, &guard);
        if !deliver(&tx, event, &stopping) {
            return; // receiver dropped, or we are shutting down
        }
    }
}

fn current_change_count() -> isize {
    let _lock = pasteboard_lock();
    NSPasteboard::generalPasteboard().changeCount()
}

fn has_type(pasteboard: &NSPasteboard, name: &str) -> bool {
    let wanted = NSString::from_str(name);
    let Some(types) = pasteboard.types() else {
        return false;
    };
    types.iter().any(|t| *t == *wanted)
}

fn data_for(pasteboard: &NSPasteboard, ty: &NSString) -> Option<Vec<u8>> {
    let data: Retained<NSData> = pasteboard.dataForType(ty)?;
    let bytes = data.to_vec();
    if bytes.is_empty() { None } else { Some(bytes) }
}

fn poll_pasteboard() -> RawPoll {
    let _lock = pasteboard_lock();
    let pasteboard = NSPasteboard::generalPasteboard();

    // Checked first, and before any payload read: honouring the convention
    // means never holding a password manager's content in memory at all.
    if has_type(&pasteboard, CONCEALED_TYPE) || has_type(&pasteboard, TRANSIENT_TYPE) {
        return RawPoll::Concealed;
    }

    let mut payloads = Vec::new();

    // The type constants are `unsafe` statics in objc2-app-kit (reading a
    // non-Rust global), while the methods around them are safe.
    if let Some(bytes) = data_for(&pasteboard, unsafe { NSPasteboardTypeString }) {
        payloads.push((ClipFormat::Text, bytes));
    }
    if let Some(bytes) = data_for(&pasteboard, unsafe { NSPasteboardTypeHTML }) {
        payloads.push((ClipFormat::Html, bytes));
    }
    if let Some(bytes) = data_for(&pasteboard, unsafe { NSPasteboardTypeRTF }) {
        payloads.push((ClipFormat::Rtf, bytes));
    }
    if let Some(bytes) = data_for(&pasteboard, unsafe { NSPasteboardTypePNG }) {
        payloads.push((ClipFormat::Png, bytes));
    } else if let Some(bytes) = data_for(&pasteboard, unsafe { NSPasteboardTypeTIFF }) {
        // Screenshots and many apps put TIFF on the pasteboard and nothing
        // else. Carried through as an opaque representation rather than
        // transcoded here: this crate does not depend on an image codec, and
        // re-encoding would change the bytes the user copied.
        payloads.push((ClipFormat::Other("image/tiff".into()), bytes));
    }

    if let Some(list) = file_urls(&pasteboard) {
        payloads.push((ClipFormat::FileList, list));
    }

    if payloads.is_empty() {
        return RawPoll::Empty;
    }

    RawPoll::Data(Capture {
        payloads,
        app: frontmost_app(),
    })
}

/// One `public.file-url` per pasteboard item, joined with newlines to match
/// the `ClipFormat::FileList` convention the Windows backend also produces.
fn file_urls(pasteboard: &NSPasteboard) -> Option<Vec<u8>> {
    let items = pasteboard.pasteboardItems()?;
    let ty = NSString::from_str(FILE_URL_TYPE);

    let paths: Vec<String> = items
        .iter()
        .filter_map(|item| item.stringForType(&ty))
        .map(|s| s.to_string())
        .collect();

    if paths.is_empty() {
        None
    } else {
        Some(paths.join("\n").into_bytes())
    }
}

/// Best-effort. macOS does not expose the pasteboard's owner, so the
/// frontmost application is the closest honest approximation — it is right
/// whenever the user just pressed Cmd+C, which is the case that matters for
/// the app blocklist.
fn frontmost_app() -> Option<String> {
    use objc2_app_kit::NSWorkspace;

    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    let name = app.localizedName()?;
    Some(name.to_string())
}

struct MacClipboard {
    guard: Arc<OwnWriteGuard>,
}

impl Clipboard for MacClipboard {
    fn read(&self) -> Result<Option<Capture>> {
        match poll_pasteboard() {
            RawPoll::Data(capture) => Ok(Some(capture)),
            RawPoll::Concealed | RawPoll::Empty => Ok(None),
        }
    }

    fn write(&self, payloads: &[(ClipFormat, Vec<u8>)]) -> Result<()> {
        {
            let _lock = pasteboard_lock();
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();

            for (format, bytes) in payloads {
                let Some(ty) = write_type_for(format) else {
                    // Formats macOS has no pasteboard type for are skipped
                    // rather than failing the whole write: a paste that keeps
                    // the text but drops an exotic representation is better
                    // than a paste that does not happen.
                    continue;
                };
                let data = NSData::with_bytes(bytes);
                let ok = pasteboard.setData_forType(Some(&data), &ty);
                if !ok {
                    return Err(Error::Platform(format!(
                        "NSPasteboard setData:forType: rejected {}",
                        format.label()
                    )));
                }
            }
        }

        self.guard.record_write(hash_payloads(payloads));
        Ok(())
    }
}

/// The AppKit type constants are `&'static NSString` while the file-URL type
/// has to be built at runtime, so everything is copied into an owned
/// `Retained` to give the branches one type.
fn write_type_for(format: &ClipFormat) -> Option<Retained<NSString>> {
    let name: &NSString = unsafe {
        match format {
            ClipFormat::Text => NSPasteboardTypeString,
            ClipFormat::Html => NSPasteboardTypeHTML,
            ClipFormat::Rtf => NSPasteboardTypeRTF,
            ClipFormat::Png => NSPasteboardTypePNG,
            ClipFormat::FileList => return Some(NSString::from_str(FILE_URL_TYPE)),
            ClipFormat::Other(name) if name == "image/tiff" => NSPasteboardTypeTIFF,
            ClipFormat::Jpeg | ClipFormat::Svg | ClipFormat::Other(_) => return None,
        }
    };
    Some(name.copy())
}
