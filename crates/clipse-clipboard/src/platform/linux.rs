//! Linux backend. Two display servers, one entry point.
//!
//! * **X11** — XFIXES `SelectionNotify` on `CLIPBOARD` tells us when the
//!   selection owner changes. Reading and writing go through `x11-clipboard`,
//!   which implements the INCR protocol that anything larger than one X
//!   request needs.
//! * **Wayland** — `wlr-data-control`, via `wl-clipboard-rs`. This protocol is
//!   how a clipboard manager is *supposed* to work on Wayland; wlroots
//!   compositors (Sway, Hyprland, river) and KDE implement it.
//! * **GNOME Wayland** — Mutter implements neither `wlr-data-control` nor any
//!   equivalent, so there is no way to observe the clipboard in the
//!   background. Clipse reports [`WatchMode::ManualPush`] with a reason the UI
//!   shows verbatim, rather than silently capturing nothing.
//!
//! None of this has been exercised on a real Linux desktop. It is type-checked
//! against the Linux target and CI builds and tests it, but the manual pass in
//! `docs/manual-verification.md` has not been run on Linux — treat the runtime
//! behaviour here as unproven.

use std::sync::Arc;
use std::time::Duration;

use clipse_core::ClipFormat;
use tokio::sync::mpsc;
use tracing::debug;

use crate::capture::{Capture, Clipboard};
use crate::error::{Error, Result};
use crate::own_write_guard::OwnWriteGuard;
use crate::watch::{CaptureEvent, WatchConfig, WatchMode, Watcher};

/// How long to wait for a selection owner to answer a conversion request.
/// Generous on purpose: the owner may be a slow Electron app, and a missed
/// paste is worse than a briefly blocked watch thread.
const SELECTION_TIMEOUT: Duration = Duration::from_millis(500);

/// Granularity of the X11 shutdown check. This polls the X *socket*, not the
/// clipboard — XFIXES still delivers its notification immediately; the tick
/// only bounds how long dropping the watcher takes to join the thread.
const X11_POLL_TICK: Duration = Duration::from_millis(50);

/// Wayland content-comparison interval. Matches the macOS cadence for the
/// same reason: fast enough to feel instant, slow enough to be invisible.
const WAYLAND_POLL_TICK: Duration = Duration::from_millis(250);

pub(crate) fn start(
    config: WatchConfig,
    guard: Arc<OwnWriteGuard>,
    tx: mpsc::Sender<CaptureEvent>,
) -> Result<Watcher> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        match wayland::probe() {
            Ok(()) => return wayland::start(config, guard, tx),
            Err(reason) => {
                debug!(%reason, "wlr-data-control unavailable; falling back");
                return degraded(reason, guard);
            }
        }
    }

    if std::env::var_os("DISPLAY").is_some() {
        return x11::start(config, guard, tx);
    }

    Err(Error::WatcherStartup(
        "neither WAYLAND_DISPLAY nor DISPLAY is set — no display server to watch".into(),
    ))
}

/// Wayland session without `wlr-data-control`. If XWayland is up we can still
/// read and write on demand — which is what makes the popup and an explicit
/// "push this clip" work — we just cannot be *notified* of changes.
fn degraded(reason: String, guard: Arc<OwnWriteGuard>) -> Result<Watcher> {
    let backend: Arc<dyn Clipboard> = if std::env::var_os("DISPLAY").is_some() {
        Arc::new(x11::X11Clipboard::new(guard)?)
    } else {
        Arc::new(UnavailableClipboard)
    };

    Ok(Watcher::new(
        WatchMode::ManualPush { reason },
        backend,
        Box::new(|| {}),
    ))
}

/// Stands in when there is no usable clipboard at all, so the daemon still
/// starts and the UI can explain the situation instead of the process dying.
struct UnavailableClipboard;

impl Clipboard for UnavailableClipboard {
    fn read(&self) -> Result<Option<Capture>> {
        Ok(None)
    }

    fn write(&self, _payloads: &[(ClipFormat, Vec<u8>)]) -> Result<()> {
        Err(Error::Platform(
            "this Wayland compositor does not let a background app set the clipboard".into(),
        ))
    }
}

// ---------------------------------------------------------------- X11 -----

mod x11 {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use clipse_core::ClipFormat;
    use tokio::sync::mpsc;
    use tracing::warn;
    use x11rb::connection::Connection;
    use x11rb::protocol::Event as X11Event;
    use x11rb::protocol::xfixes::{ConnectionExt as XfixesExt, SelectionEventMask};
    use x11rb::protocol::xproto::{Atom, ConnectionExt as XprotoExt};
    use x11rb::rust_connection::RustConnection;

    use super::{SELECTION_TIMEOUT, X11_POLL_TICK};
    use crate::capture::{Capture, Clipboard, hash_payloads};
    use crate::error::{Error, Result};
    use crate::own_write_guard::OwnWriteGuard;
    use crate::watch::{CaptureEvent, RawPoll, WatchConfig, WatchMode, Watcher, classify, deliver};

    /// KDE's cross-desktop convention: a selection owner advertises this
    /// target with the value `secret` to tell clipboard managers to ignore the
    /// selection. Honoured the same way as the Windows and macOS equivalents.
    const PASSWORD_HINT_TARGET: &str = "x-kde-passwordManagerHint";

    pub(super) fn start(
        config: WatchConfig,
        guard: Arc<OwnWriteGuard>,
        tx: mpsc::Sender<CaptureEvent>,
    ) -> Result<Watcher> {
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_stopping = Arc::clone(&stopping);
        let thread_guard = Arc::clone(&guard);

        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
        let join = std::thread::Builder::new()
            .name("clipse-clipboard-watch".into())
            .spawn(move || event_loop(config, thread_guard, tx, thread_stopping, ready_tx))
            .map_err(|e| Error::WatcherStartup(format!("failed to spawn watch thread: {e}")))?;

        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(Error::WatcherStartup(
                    "watch thread exited before it finished starting up".into(),
                ));
            }
        }

        let backend: Arc<dyn Clipboard> = Arc::new(X11Clipboard::new(guard)?);
        let mut join = Some(join);
        let stopper: Box<dyn FnOnce() + Send + Sync> = Box::new(move || {
            stopping.store(true, Ordering::SeqCst);
            if let Some(handle) = join.take() {
                let _ = handle.join();
            }
        });

        Ok(Watcher::new(WatchMode::Automatic, backend, stopper))
    }

    fn event_loop(
        config: WatchConfig,
        guard: Arc<OwnWriteGuard>,
        tx: mpsc::Sender<CaptureEvent>,
        stopping: Arc<AtomicBool>,
        ready: std::sync::mpsc::Sender<Result<()>>,
    ) {
        let watch_conn = match connect_and_subscribe() {
            Ok(c) => c,
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        if ready.send(Ok(())).is_err() {
            return;
        }

        // Converting the selection needs its own connection: the watch
        // connection is mid-event-delivery when we want to round-trip.
        let reader = match SelectionReader::new() {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "clipboard reader connection failed; watch is inert");
                return;
            }
        };

        while !stopping.load(Ordering::SeqCst) {
            match watch_conn.poll_for_event() {
                Ok(Some(X11Event::XfixesSelectionNotify(_))) => {
                    let event = classify(reader.poll(), &config, &guard);
                    if !deliver(&tx, event, &stopping) {
                        return;
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => std::thread::sleep(X11_POLL_TICK),
                Err(e) => {
                    warn!(error = %e, "X11 connection lost; stopping clipboard watch");
                    return;
                }
            }
        }
    }

    fn connect_and_subscribe() -> Result<RustConnection> {
        let (conn, screen_num) =
            x11rb::connect(None).map_err(|e| Error::Platform(format!("X11 connect: {e}")))?;

        // XFIXES has to be negotiated before any of its requests are legal.
        conn.xfixes_query_version(5, 0)
            .map_err(|e| Error::Platform(format!("XFIXES unavailable: {e}")))?
            .reply()
            .map_err(|e| Error::Platform(format!("XFIXES unavailable: {e}")))?;

        let root = conn
            .setup()
            .roots
            .get(screen_num)
            .ok_or_else(|| Error::Platform("X11 screen out of range".into()))?
            .root;
        let clipboard = intern(&conn, "CLIPBOARD")?;

        conn.xfixes_select_selection_input(
            root,
            clipboard,
            SelectionEventMask::SET_SELECTION_OWNER,
        )
        .map_err(|e| Error::Platform(format!("XFIXES SelectSelectionInput: {e}")))?
        .check()
        .map_err(|e| Error::Platform(format!("XFIXES SelectSelectionInput: {e}")))?;

        conn.flush()
            .map_err(|e| Error::Platform(format!("X11 flush: {e}")))?;

        Ok(conn)
    }

    fn intern<C: Connection>(conn: &C, name: &str) -> Result<Atom> {
        Ok(conn
            .intern_atom(false, name.as_bytes())
            .map_err(|e| Error::Platform(format!("InternAtom {name}: {e}")))?
            .reply()
            .map_err(|e| Error::Platform(format!("InternAtom {name}: {e}")))?
            .atom)
    }

    /// Wraps `x11-clipboard`, which owns the conversion connection and
    /// implements INCR for transfers larger than one request.
    struct SelectionReader {
        clipboard: x11_clipboard::Clipboard,
        html: Atom,
        rtf: Atom,
        png: Atom,
        uri_list: Atom,
        password_hint: Atom,
    }

    impl SelectionReader {
        fn new() -> Result<Self> {
            let clipboard = x11_clipboard::Clipboard::new()
                .map_err(|e| Error::Platform(format!("X11 clipboard: {e}")))?;
            let conn = &clipboard.getter.connection;
            Ok(Self {
                html: intern(conn, "text/html")?,
                rtf: intern(conn, "text/rtf")?,
                png: intern(conn, "image/png")?,
                uri_list: intern(conn, "text/uri-list")?,
                password_hint: intern(conn, PASSWORD_HINT_TARGET)?,
                clipboard,
            })
        }

        fn load(&self, target: Atom) -> Option<Vec<u8>> {
            let atoms = &self.clipboard.getter.atoms;
            let bytes = self
                .clipboard
                .load(atoms.clipboard, target, atoms.property, SELECTION_TIMEOUT)
                .ok()?;
            if bytes.is_empty() { None } else { Some(bytes) }
        }

        fn poll(&self) -> RawPoll {
            // Checked before anything else, so a password manager's content is
            // never fetched into our address space at all.
            if self
                .load(self.password_hint)
                .is_some_and(|hint| hint.eq_ignore_ascii_case(b"secret"))
            {
                return RawPoll::Concealed;
            }

            let atoms = &self.clipboard.getter.atoms;
            let mut payloads = Vec::new();

            if let Some(bytes) = self.load(atoms.utf8_string) {
                payloads.push((ClipFormat::Text, bytes));
            }
            if let Some(bytes) = self.load(self.html) {
                payloads.push((ClipFormat::Html, bytes));
            }
            if let Some(bytes) = self.load(self.rtf) {
                payloads.push((ClipFormat::Rtf, bytes));
            }
            if let Some(bytes) = self.load(self.png) {
                payloads.push((ClipFormat::Png, bytes));
            }
            if let Some(bytes) = self.load(self.uri_list) {
                payloads.push((ClipFormat::FileList, uri_list_to_paths(&bytes)));
            }

            if payloads.is_empty() {
                return RawPoll::Empty;
            }

            // X11 identifies a selection owner by window id, which rarely maps
            // back to a name a user would recognise, so the app blocklist does
            // not apply here. The concealed-target check above and the secret
            // detectors carry the privacy guarantee on this platform.
            RawPoll::Data(Capture {
                payloads,
                app: None,
            })
        }
    }

    pub(super) struct X11Clipboard {
        reader: SelectionReader,
        guard: Arc<OwnWriteGuard>,
    }

    impl X11Clipboard {
        pub(super) fn new(guard: Arc<OwnWriteGuard>) -> Result<Self> {
            Ok(Self {
                reader: SelectionReader::new()?,
                guard,
            })
        }
    }

    impl Clipboard for X11Clipboard {
        fn read(&self) -> Result<Option<Capture>> {
            match self.reader.poll() {
                RawPoll::Data(capture) => Ok(Some(capture)),
                RawPoll::Concealed | RawPoll::Empty => Ok(None),
            }
        }

        fn write(&self, payloads: &[(ClipFormat, Vec<u8>)]) -> Result<()> {
            // X11 selection ownership is per-owner, not per-target: claiming
            // the selection for one target replaces everything that was there.
            // Plain text therefore wins, because it is the representation every
            // paste target understands. Serving several targets at once would
            // mean running our own selection-request loop instead of using
            // x11-clipboard, which is a trade worth making only once someone
            // can test it on a real desktop.
            let chosen = payloads
                .iter()
                .find(|(f, _)| *f == ClipFormat::Text)
                .or_else(|| payloads.first());

            let Some((format, bytes)) = chosen else {
                return Ok(());
            };

            let atoms = &self.reader.clipboard.setter.atoms;
            let target = match format {
                ClipFormat::Html => self.reader.html,
                ClipFormat::Rtf => self.reader.rtf,
                ClipFormat::Png => self.reader.png,
                ClipFormat::FileList => self.reader.uri_list,
                ClipFormat::Text | ClipFormat::Jpeg | ClipFormat::Svg | ClipFormat::Other(_) => {
                    atoms.utf8_string
                }
            };

            self.reader
                .clipboard
                .store(atoms.clipboard, target, bytes.as_slice())
                .map_err(|e| Error::Platform(format!("X11 selection store: {e}")))?;

            self.guard.record_write(hash_payloads(payloads));
            Ok(())
        }
    }

    fn uri_list_to_paths(bytes: &[u8]) -> Vec<u8> {
        // `text/uri-list` is CRLF-separated and allows `#` comment lines; the
        // FileList convention across this crate is newline-separated entries.
        String::from_utf8_lossy(bytes)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes()
    }
}

// ------------------------------------------------------------ Wayland -----

mod wayland {
    use std::io::Read;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use clipse_core::ClipFormat;
    use tokio::sync::mpsc;
    use wl_clipboard_rs::copy::{MimeType as CopyMime, Options, Source};
    use wl_clipboard_rs::paste::{ClipboardType, MimeType as PasteMime, Seat, get_contents};

    use super::WAYLAND_POLL_TICK;
    use crate::capture::{Capture, Clipboard, hash_payloads};
    use crate::error::{Error, Result};
    use crate::own_write_guard::OwnWriteGuard;
    use crate::watch::{CaptureEvent, RawPoll, WatchConfig, WatchMode, Watcher, classify, deliver};

    /// Same cross-desktop convention as on X11.
    const PASSWORD_HINT_MIME: &str = "x-kde-passwordManagerHint";

    /// Is `wlr-data-control` available on this compositor?
    ///
    /// `wl-clipboard-rs` reports a missing protocol as its own error variant,
    /// which is exactly the GNOME/Mutter case we need to distinguish from a
    /// genuine failure.
    pub(super) fn probe() -> std::result::Result<(), String> {
        use wl_clipboard_rs::utils::PrimarySelectionCheckError;

        match wl_clipboard_rs::utils::is_primary_selection_supported() {
            // A compositor with data-control but no seat yet is still fine —
            // seats appear when an input device does.
            Ok(_) | Err(PrimarySelectionCheckError::NoSeats) => Ok(()),
            Err(e) => Err(format!(
                "This desktop does not let background apps watch the clipboard ({e}). \
                 GNOME on Wayland is the usual reason. Clipse will still sync what \
                 arrives from your other devices; to send a copy from here, use the \
                 Clipse hotkey and push it manually."
            )),
        }
    }

    pub(super) fn start(
        config: WatchConfig,
        guard: Arc<OwnWriteGuard>,
        tx: mpsc::Sender<CaptureEvent>,
    ) -> Result<Watcher> {
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_stopping = Arc::clone(&stopping);
        let thread_guard = Arc::clone(&guard);

        let join = std::thread::Builder::new()
            .name("clipse-clipboard-watch".into())
            .spawn(move || poll_loop(config, thread_guard, tx, thread_stopping))
            .map_err(|e| Error::WatcherStartup(format!("failed to spawn watch thread: {e}")))?;

        let backend: Arc<dyn Clipboard> = Arc::new(WaylandClipboard { guard });
        let mut join = Some(join);
        let stopper: Box<dyn FnOnce() + Send + Sync> = Box::new(move || {
            stopping.store(true, Ordering::SeqCst);
            if let Some(handle) = join.take() {
                let _ = handle.join();
            }
        });

        Ok(Watcher::new(WatchMode::Automatic, backend, stopper))
    }

    /// `wl-clipboard-rs` exposes a one-shot read rather than a change stream,
    /// so change detection here is a content comparison on a timer instead of
    /// a data-control listener. That is more work per tick than the protocol
    /// requires; the honest reason it is done this way is that a hand-written
    /// listener could not be run on any machine available while writing this.
    /// Worth replacing once someone can test it on Sway or KDE.
    fn poll_loop(
        config: WatchConfig,
        guard: Arc<OwnWriteGuard>,
        tx: mpsc::Sender<CaptureEvent>,
        stopping: Arc<AtomicBool>,
    ) {
        // Seeded so whatever was already on the clipboard at startup is not
        // captured as if the user had just copied it.
        let mut last = read_clipboard();

        while !stopping.load(Ordering::SeqCst) {
            std::thread::sleep(WAYLAND_POLL_TICK);
            if stopping.load(Ordering::SeqCst) {
                return;
            }

            let current = read_clipboard();
            if same(&current, &last) {
                continue;
            }
            last = clone_poll(&current);

            let event = classify(current, &config, &guard);
            if !deliver(&tx, event, &stopping) {
                return;
            }
        }
    }

    fn same(a: &RawPoll, b: &RawPoll) -> bool {
        match (a, b) {
            (RawPoll::Empty, RawPoll::Empty) | (RawPoll::Concealed, RawPoll::Concealed) => true,
            (RawPoll::Data(x), RawPoll::Data(y)) => x.content_hash() == y.content_hash(),
            _ => false,
        }
    }

    fn clone_poll(poll: &RawPoll) -> RawPoll {
        match poll {
            RawPoll::Empty => RawPoll::Empty,
            RawPoll::Concealed => RawPoll::Concealed,
            RawPoll::Data(c) => RawPoll::Data(c.clone()),
        }
    }

    fn read_mime(mime: &str) -> Option<Vec<u8>> {
        let (mut pipe, _) = get_contents(
            ClipboardType::Regular,
            Seat::Unspecified,
            PasteMime::Specific(mime),
        )
        .ok()?;
        let mut buf = Vec::new();
        pipe.read_to_end(&mut buf).ok()?;
        if buf.is_empty() { None } else { Some(buf) }
    }

    fn read_clipboard() -> RawPoll {
        if read_mime(PASSWORD_HINT_MIME).is_some_and(|hint| hint.eq_ignore_ascii_case(b"secret")) {
            return RawPoll::Concealed;
        }

        let mut payloads = Vec::new();
        for (mime, format) in [
            ("text/plain;charset=utf-8", ClipFormat::Text),
            ("text/html", ClipFormat::Html),
            ("text/rtf", ClipFormat::Rtf),
            ("image/png", ClipFormat::Png),
            ("text/uri-list", ClipFormat::FileList),
        ] {
            if let Some(bytes) = read_mime(mime) {
                payloads.push((format, bytes));
            }
        }

        if payloads.is_empty() {
            return RawPoll::Empty;
        }

        // wlr-data-control does not name the offering client, so as on X11 the
        // app blocklist cannot apply here.
        RawPoll::Data(Capture {
            payloads,
            app: None,
        })
    }

    struct WaylandClipboard {
        guard: Arc<OwnWriteGuard>,
    }

    impl Clipboard for WaylandClipboard {
        fn read(&self) -> Result<Option<Capture>> {
            match read_clipboard() {
                RawPoll::Data(capture) => Ok(Some(capture)),
                RawPoll::Concealed | RawPoll::Empty => Ok(None),
            }
        }

        fn write(&self, payloads: &[(ClipFormat, Vec<u8>)]) -> Result<()> {
            let chosen = payloads
                .iter()
                .find(|(f, _)| *f == ClipFormat::Text)
                .or_else(|| payloads.first());
            let Some((format, bytes)) = chosen else {
                return Ok(());
            };

            let mime = match format {
                ClipFormat::Text => "text/plain;charset=utf-8",
                ClipFormat::Html => "text/html",
                ClipFormat::Rtf => "text/rtf",
                ClipFormat::Png => "image/png",
                ClipFormat::Jpeg => "image/jpeg",
                ClipFormat::Svg => "image/svg+xml",
                ClipFormat::FileList => "text/uri-list",
                ClipFormat::Other(name) => name.as_str(),
            };

            let mut options = Options::new();
            // The copy has to outlive this call: on Wayland the *source client*
            // serves the data when a paste happens, so a foreground copy would
            // block until someone pasted.
            options.foreground(false);
            options
                .copy(
                    Source::Bytes(bytes.clone().into_boxed_slice()),
                    CopyMime::Specific(mime.to_string()),
                )
                .map_err(|e| Error::Platform(format!("wayland copy: {e}")))?;

            self.guard.record_write(hash_payloads(payloads));
            Ok(())
        }
    }
}
