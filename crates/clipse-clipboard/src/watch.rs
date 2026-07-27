use std::sync::Arc;
use std::time::Duration;

use clipse_core::ClipFormat;
use tokio::sync::mpsc;

use crate::capture::{Capture, Clipboard};
use crate::error::Result;
use crate::own_write_guard::{DEFAULT_OWN_WRITE_TTL, OwnWriteGuard};
use crate::sensitive::{AppBlocklist, SecretKind, detect_secret};

/// Ceiling on total bytes across a capture's representations before it is
/// rejected outright. Far above `clipse_core::INLINE_MAX_BYTES` (64 KiB,
/// which only decides inline-vs-blob storage) — this is a sanity backstop
/// against a pathological clipboard write (e.g. an uncompressed multi-monitor
/// screenshot bitmap) filling the store or the sync channel with one clip.
pub const DEFAULT_MAX_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;

/// Reason a candidate clipboard change was not turned into a capture.
///
/// Carries no clipboard content — only enough to log *that* something was
/// suppressed and roughly *why*, per the rule that suppressed content must
/// never reach a log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuppressionReason {
    /// The platform marked this as a password-manager copy
    /// (`ExcludeClipboardContentFromMonitorProcessing` on Windows,
    /// `org.nspasteboard.ConcealedType`/`TransientType` on macOS,
    /// `x-kde-passwordManagerHint=secret` on X11).
    ConcealedFormat,
    BlockedApp(String),
    DetectedSecret(SecretKind),
    Empty,
    TooLarge {
        bytes: u64,
    },
    /// This is the echo of our own `Clipboard::write` — see `OwnWriteGuard`.
    OwnWrite,
}

/// Either an accepted capture or the reason it was suppressed.
///
/// `Debug` is implemented by hand below (rather than derived) so that
/// logging a `CaptureEvent` — which the daemon does for every suppression —
/// can never accidentally print clipboard bytes.
#[derive(Clone)]
pub enum CaptureEvent {
    Captured(Capture),
    Suppressed(SuppressionReason),
}

/// Whether the watcher can see clipboard changes as they happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchMode {
    Automatic,
    /// No background clipboard monitoring is possible — GNOME's Mutter is
    /// the motivating case, since it does not implement `wlr-data-control`.
    /// `reason` is meant to be shown to the user as-is.
    ManualPush {
        reason: String,
    },
}

#[derive(Clone, Debug)]
pub struct WatchConfig {
    pub app_blocklist: AppBlocklist,
    pub max_payload_bytes: u64,
    pub detect_secrets: bool,
    /// How long the `OwnWriteGuard` waits for a write's echo before giving up
    /// on it. See `own_write_guard` for why this exists at all.
    pub own_write_ttl: Duration,
    /// Backpressure limit on the event channel. Small on purpose: a slow
    /// consumer should feel backpressure (and the watcher should log that it
    /// is falling behind) rather than let captures pile up unbounded in
    /// memory, which for clipboard content could mean buffering secrets.
    pub channel_capacity: usize,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            app_blocklist: AppBlocklist::defaults(),
            max_payload_bytes: DEFAULT_MAX_PAYLOAD_BYTES,
            detect_secrets: true,
            own_write_ttl: DEFAULT_OWN_WRITE_TTL,
            channel_capacity: 16,
        }
    }
}

/// Raw outcome of one platform poll/notification, before the shared
/// suppression pipeline runs. Platform-internal: callers only ever see the
/// resulting `CaptureEvent`.
pub(crate) enum RawPoll {
    /// The platform-specific "this is concealed" marker was present. We do
    /// not even attempt to read payloads in this case — reading them would
    /// mean briefly holding password-manager content in memory for no
    /// reason.
    Concealed,
    Empty,
    Data(Capture),
}

/// Shared suppression pipeline: every platform backend funnels its raw poll
/// result through this, so the privacy rules are defined and tested exactly
/// once regardless of how many platforms end up implemented.
pub(crate) fn classify(poll: RawPoll, config: &WatchConfig, guard: &OwnWriteGuard) -> CaptureEvent {
    let capture = match poll {
        RawPoll::Concealed => return CaptureEvent::Suppressed(SuppressionReason::ConcealedFormat),
        RawPoll::Empty => return CaptureEvent::Suppressed(SuppressionReason::Empty),
        RawPoll::Data(capture) if capture.payloads.is_empty() => {
            return CaptureEvent::Suppressed(SuppressionReason::Empty);
        }
        RawPoll::Data(capture) => capture,
    };

    let total_bytes = capture.total_bytes();
    if total_bytes > config.max_payload_bytes {
        return CaptureEvent::Suppressed(SuppressionReason::TooLarge { bytes: total_bytes });
    }

    if let Some(app) = capture.app.as_deref()
        && config.app_blocklist.is_blocked(app)
    {
        return CaptureEvent::Suppressed(SuppressionReason::BlockedApp(app.to_string()));
    }

    // Checked before secret-scanning: an echoed remote clip was already
    // scanned (or explicitly trusted) by the sending device, and re-scanning
    // it here buys nothing but CPU.
    if guard.check(capture.content_hash()) {
        return CaptureEvent::Suppressed(SuppressionReason::OwnWrite);
    }

    if config.detect_secrets
        && let Some(kind) = detect_secret_in_payloads(&capture.payloads)
    {
        return CaptureEvent::Suppressed(SuppressionReason::DetectedSecret(kind));
    }

    CaptureEvent::Captured(capture)
}

fn detect_secret_in_payloads(payloads: &[(ClipFormat, Vec<u8>)]) -> Option<SecretKind> {
    payloads.iter().find_map(|(format, bytes)| {
        let is_text_like = matches!(
            format,
            ClipFormat::Text | ClipFormat::Html | ClipFormat::Rtf
        );
        if !is_text_like {
            return None;
        }
        // Lossy on purpose: RTF and HTML fragments can carry control words
        // or entities that are not themselves valid UTF-8 sequences, but a
        // secret's characters are always plain ASCII, so a lossy decode
        // still lets every detector above match.
        detect_secret(&String::from_utf8_lossy(bytes))
    })
}

/// Hand an event to the consumer, staying responsive to shutdown.
///
/// A plain `blocking_send` deadlocks on teardown: the watch thread parks on a
/// full channel, while the thread calling `drop(watcher)` parks in `join`
/// waiting for it — and the only thread that could drain the channel is the
/// one now stuck in `join`. Polling `try_send` and re-checking `stopping`
/// keeps the backpressure (a slow UI still slows the watcher rather than
/// letting captures pile up in memory) without that cycle.
///
/// Returns false when the watch loop should exit.
pub(crate) fn deliver(
    tx: &mpsc::Sender<CaptureEvent>,
    mut event: CaptureEvent,
    stopping: &std::sync::atomic::AtomicBool,
) -> bool {
    use std::sync::atomic::Ordering;
    use tokio::sync::mpsc::error::TrySendError;

    loop {
        if stopping.load(Ordering::SeqCst) {
            return false;
        }
        match tx.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Closed(_)) => return false,
            Err(TrySendError::Full(returned)) => {
                event = returned;
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// Handle to a running watcher. Dropping it stops the platform watch loop
/// (window message loop thread on Windows, poll timer on macOS, event loop on
/// Linux) and joins it, so no capture arrives after the handle is gone.
///
/// Also implements `Clipboard`: writes go through the same backend instance
/// (and the same `OwnWriteGuard`) that the watch loop reads from, which is
/// what makes the own-write suppression actually fire for the *next*
/// capture after a write.
pub struct Watcher {
    mode: WatchMode,
    backend: Arc<dyn Clipboard>,
    // `+ Sync` (in addition to the `FnOnce`-implied `Send`) so `Watcher`
    // itself satisfies `Clipboard: Send + Sync` — nothing ever calls this
    // through a shared reference, it is just a marker the contained state
    // (a join handle and a raw window handle) already satisfies.
    stopper: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl Watcher {
    pub(crate) fn new(
        mode: WatchMode,
        backend: Arc<dyn Clipboard>,
        stopper: Box<dyn FnOnce() + Send + Sync>,
    ) -> Self {
        Self {
            mode,
            backend,
            stopper: Some(stopper),
        }
    }

    pub fn mode(&self) -> &WatchMode {
        &self.mode
    }
}

impl Clipboard for Watcher {
    fn read(&self) -> Result<Option<Capture>> {
        self.backend.read()
    }

    fn write(&self, payloads: &[(ClipFormat, Vec<u8>)]) -> Result<()> {
        self.backend.write(payloads)
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        if let Some(stop) = self.stopper.take() {
            stop();
        }
    }
}

/// Start watching the OS clipboard. Returns a handle; dropping it stops the
/// watcher. Events (accepted captures and suppressions alike) arrive on the
/// returned channel until then.
pub fn watch(config: WatchConfig) -> Result<(Watcher, mpsc::Receiver<CaptureEvent>)> {
    let (tx, rx) = mpsc::channel(config.channel_capacity);
    let guard = Arc::new(OwnWriteGuard::new(config.own_write_ttl));
    let watcher = crate::platform::start(config, guard, tx)?;
    Ok((watcher, rx))
}

impl std::fmt::Debug for CaptureEvent {
    // Manual impl (instead of derive) so the invariant is explicit here
    // rather than relying on `Capture`'s derived `Debug`: if a future field
    // is added to `Capture` that should stay out of logs, this is the one
    // place that needs revisiting, not every call site that logs a
    // `CaptureEvent`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Captured(c) => f
                .debug_struct("Captured")
                .field(
                    "formats",
                    &c.payloads
                        .iter()
                        .map(|(f, _)| f.label())
                        .collect::<Vec<_>>(),
                )
                .field("bytes", &c.total_bytes())
                .finish(),
            Self::Suppressed(reason) => f.debug_tuple("Suppressed").field(reason).finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use clipse_core::ContentHash;

    use super::*;

    fn config() -> WatchConfig {
        WatchConfig::default()
    }

    fn text_capture(s: &str) -> Capture {
        Capture {
            payloads: vec![(ClipFormat::Text, s.as_bytes().to_vec())],
            app: None,
        }
    }

    #[test]
    fn concealed_marker_suppresses_without_reading_content() {
        let event = classify(RawPoll::Concealed, &config(), &OwnWriteGuard::default());
        assert!(matches!(
            event,
            CaptureEvent::Suppressed(SuppressionReason::ConcealedFormat)
        ));
    }

    #[test]
    fn empty_clipboard_is_suppressed() {
        let event = classify(RawPoll::Empty, &config(), &OwnWriteGuard::default());
        assert!(matches!(
            event,
            CaptureEvent::Suppressed(SuppressionReason::Empty)
        ));

        let empty_capture = Capture {
            payloads: vec![],
            app: None,
        };
        let event = classify(
            RawPoll::Data(empty_capture),
            &config(),
            &OwnWriteGuard::default(),
        );
        assert!(matches!(
            event,
            CaptureEvent::Suppressed(SuppressionReason::Empty)
        ));
    }

    #[test]
    fn oversized_capture_is_suppressed() {
        let mut cfg = config();
        cfg.max_payload_bytes = 10;
        let capture = text_capture("this is way more than ten bytes");
        let event = classify(RawPoll::Data(capture), &cfg, &OwnWriteGuard::default());
        assert!(matches!(
            event,
            CaptureEvent::Suppressed(SuppressionReason::TooLarge { .. })
        ));
    }

    #[test]
    fn blocked_app_is_suppressed() {
        let cfg = config();
        let capture = Capture {
            payloads: vec![(ClipFormat::Text, b"my master password".to_vec())],
            app: Some("1Password.exe".to_string()),
        };
        let event = classify(RawPoll::Data(capture), &cfg, &OwnWriteGuard::default());
        assert!(matches!(
            event,
            CaptureEvent::Suppressed(SuppressionReason::BlockedApp(app)) if app == "1Password.exe"
        ));
    }

    #[test]
    fn secret_content_is_suppressed() {
        let capture = text_capture("AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE");
        let event = classify(RawPoll::Data(capture), &config(), &OwnWriteGuard::default());
        assert!(matches!(
            event,
            CaptureEvent::Suppressed(SuppressionReason::DetectedSecret(SecretKind::AwsAccessKey))
        ));
    }

    #[test]
    fn secret_detection_can_be_turned_off() {
        let mut cfg = config();
        cfg.detect_secrets = false;
        let capture = text_capture("AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE");
        let event = classify(RawPoll::Data(capture), &cfg, &OwnWriteGuard::default());
        assert!(matches!(event, CaptureEvent::Captured(_)));
    }

    #[test]
    fn own_write_echo_is_suppressed_and_only_once() {
        let guard = OwnWriteGuard::default();
        let capture = text_capture("round tripped from a peer");
        guard.record_write(capture.content_hash());

        let event = classify(RawPoll::Data(capture.clone()), &config(), &guard);
        assert!(matches!(
            event,
            CaptureEvent::Suppressed(SuppressionReason::OwnWrite)
        ));

        // A second, later capture of identical content is a genuine re-copy.
        let event = classify(RawPoll::Data(capture), &config(), &guard);
        assert!(matches!(event, CaptureEvent::Captured(_)));
    }

    #[test]
    fn ordinary_capture_is_accepted() {
        let capture = text_capture("just a normal clipboard entry");
        let event = classify(
            RawPoll::Data(capture.clone()),
            &config(),
            &OwnWriteGuard::default(),
        );
        match event {
            CaptureEvent::Captured(c) => assert_eq!(c, capture),
            other => panic!("expected Captured, got {other:?}"),
        }
    }

    #[test]
    fn suppression_checks_run_in_a_privacy_preserving_order() {
        // A blocked app AND a secret both apply; either suppression reason
        // is correct, but it must be *some* suppression, never Captured —
        // guards against a future reordering accidentally short-circuiting
        // past every check.
        let capture = Capture {
            payloads: vec![(
                ClipFormat::Text,
                [b"sk".as_slice(), b"_live_4eC39HqLyjWDarjtT1zdp7dc"].concat(),
            )],
            app: Some("Bitwarden".to_string()),
        };
        let event = classify(RawPoll::Data(capture), &config(), &OwnWriteGuard::default());
        assert!(matches!(event, CaptureEvent::Suppressed(_)));
    }

    #[test]
    fn content_hash_helper_matches_capture_method() {
        let capture = text_capture("consistency check");
        assert_eq!(
            capture.content_hash(),
            ContentHash::of_parts(&[("text/plain", b"consistency check")])
        );
    }
}
