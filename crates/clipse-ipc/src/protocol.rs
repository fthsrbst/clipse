use clipse_core::{Clip, ClipFormat, ClipId, ClipKind, DeviceId};
use serde::{Deserialize, Serialize};

/// One message on the wire. `id` correlates a [`Response`] with its
/// [`Request`]; events carry `id == 0` because nobody asked for them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub id: u64,
    pub body: FrameBody,
}

impl Frame {
    pub fn request(id: u64, request: Request) -> Self {
        Self {
            id,
            body: FrameBody::Request(request),
        }
    }

    pub fn response(id: u64, response: Response) -> Self {
        Self {
            id,
            body: FrameBody::Response(response),
        }
    }

    pub fn event(event: Event) -> Self {
        Self {
            id: 0,
            body: FrameBody::Event(event),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FrameBody {
    Request(Request),
    Response(Response),
    Event(Event),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HistoryQuery {
    pub limit: u32,
    pub offset: u32,
    pub kind: Option<ClipKind>,
    pub pinned_only: bool,
}

impl HistoryQuery {
    pub fn page(limit: u32) -> Self {
        Self {
            limit,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    /// Always the first message. Lets both sides fail loudly on a version
    /// mismatch instead of misparsing each other's frames later.
    Hello {
        client: String,
        ipc_version: u16,
    },

    History(HistoryQuery),
    Search {
        text: String,
        query: HistoryQuery,
    },
    GetClip {
        id: ClipId,
    },

    /// The bytes of one of a clip's payloads.
    ///
    /// `GetClip` cannot answer this: a payload over
    /// `clipse_core::INLINE_MAX_BYTES` has a `PayloadBody::Blob` body, which
    /// carries no bytes at all — and that covers essentially every screenshot.
    GetPayload {
        id: ClipId,
        format: ClipFormat,
    },

    /// Put a stored clip back on the local clipboard.
    Apply {
        id: ClipId,
    },
    /// Apply, then synthesise the paste keystroke into the focused window.
    Paste {
        id: ClipId,
    },

    SetPinned {
        id: ClipId,
        pinned: bool,
    },
    Delete {
        id: ClipId,
    },

    Status,
    SetPaused {
        paused: bool,
    },
    Devices,
    ForgetDevice {
        device: DeviceId,
    },

    GetSettings,
    UpdateSettings(Box<Settings>),

    /// Show a QR code: this device becomes the initiator and starts accepting
    /// one pairing attempt. Expires on its own — see `PAIRING_OFFER_TTL_SECS`.
    BeginPairing,
    /// Scan a QR code: this device answers the offer in it.
    PairWithUri {
        uri: String,
    },
    /// Commit or discard the pairing whose six digits the user just compared.
    /// The comparison is the security boundary, so nothing is trusted until
    /// this arrives.
    ConfirmPairing {
        accept: bool,
    },
    /// Stop offering to pair.
    CancelPairing,

    /// Turn this connection into an event stream as well. A UI opens one
    /// subscribed connection and keeps it for its lifetime.
    Subscribe,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    Hello {
        daemon_version: String,
        ipc_version: u16,
        device: DeviceId,
    },
    Clips(Vec<Clip>),
    Clip(Option<Box<Clip>>),
    /// `None` covers all the ordinary misses: no such clip, no payload in that
    /// format, or one past `MAX_PAYLOAD_BYTES`. None of them is an error — the
    /// caller shows a size instead of a picture.
    ///
    /// `ByteBuf` rather than `Vec<u8>` so this encodes as a MessagePack `bin`
    /// blob at 1:1. A plain byte vector encodes as an array of integers at
    /// roughly 1.5x, which would make the size cap an artefact of the encoding
    /// rather than a decision.
    PayloadBytes(Option<serde_bytes::ByteBuf>),
    Status(Box<DaemonStatus>),
    Devices(Vec<PeerInfo>),
    Settings(Box<Settings>),
    /// The string to render as a QR code, and when it stops being valid.
    PairingOffer {
        uri: String,
        expires_at_ms: u64,
    },
    /// Both devices show this. The user compares them; they must match.
    PairingCode {
        digits: String,
        peer_label: String,
    },
    Ok,
    Error(IpcError),
}

/// Pushed by the daemon to subscribed connections.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Event {
    ClipAdded(Box<Clip>),
    ClipUpdated(Box<Clip>),
    ClipRemoved(ClipId),
    StatusChanged(Box<DaemonStatus>),
    DeviceChanged(PeerInfo),
    /// A capture was dropped. Carries the *reason only* — the suppressed
    /// content must never leave the capture path, not even to a local UI.
    Suppressed {
        reason: String,
    },
    /// Someone answered our QR code. Show these digits next to theirs; the
    /// user comparing them is what makes the pairing safe.
    PairingCode {
        digits: String,
        peer_label: String,
    },
    /// The ceremony ended without a pairing — expired, refused, or failed.
    PairingEnded {
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct IpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl IpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    VersionMismatch,
    NotFound,
    BadRequest,
    Unsupported,
    Internal,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::VersionMismatch => "version_mismatch",
            Self::NotFound => "not_found",
            Self::BadRequest => "bad_request",
            Self::Unsupported => "unsupported",
            Self::Internal => "internal",
        };
        f.write_str(s)
    }
}

/// How the platform lets us observe the clipboard. `ManualPush` is not an
/// error — it is GNOME Wayland, where no compositor protocol exists for
/// background monitoring, and the UI must tell the user so.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureMode {
    Automatic,
    ManualPush { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub device: DeviceId,
    pub device_label: String,
    pub daemon_version: String,
    pub paused: bool,
    pub capture_mode: CaptureMode,
    pub clip_count: u64,
    pub blob_bytes: u64,
    pub blob_quota_bytes: u64,
    pub peers_online: u32,
    pub peers_total: u32,
    /// Captures dropped for looking like a secret, since this daemon started.
    ///
    /// A count, and only a count. Deliberately not persisted: a durable tally
    /// would mean writing a record about the thing Clipse promised not to
    /// write down, and "since Clipse started" is an honest answer that costs
    /// nothing.
    pub secrets_refused: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Connectivity {
    Lan,
    Tailnet,
    Offline,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerInfo {
    pub device: DeviceId,
    pub label: String,
    pub platform: String,
    pub connectivity: Connectivity,
    pub last_seen_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Accelerator string in Tauri's format, e.g. `CmdOrCtrl+Shift+V`.
    pub hotkey: String,
    /// When false, arriving clips land in the history but are not written to
    /// the clipboard — the "don't steal my clipboard" mode.
    pub apply_incoming_to_clipboard: bool,
    pub blob_quota_bytes: u64,
    pub blocked_apps: Vec<String>,
    pub detect_secrets: bool,
    pub sync_enabled: bool,
    /// Announce this device over mDNS so paired devices can find it again
    /// after an address change.
    ///
    /// The announcement is public to the whole local network: anyone on it can
    /// see that this machine runs Clipse, under this label. It grants nobody
    /// any access — discovery only refreshes addresses for devices that already
    /// completed a pairing — but it is presence, and on a shared network some
    /// people would rather not broadcast it. Turning it off trades automatic
    /// re-discovery for silence; paired devices are still reached at the
    /// addresses recorded when they paired.
    ///
    /// `serde(default)` so a config file or a peer written before this field
    /// existed still loads. The IPC codec keeps field names (see `codec.rs`),
    /// which is what makes adding one safe in both directions.
    #[serde(default = "announce_by_default")]
    pub announce_on_network: bool,
    pub start_at_login: bool,
    pub device_label: String,
}

fn announce_by_default() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: "CmdOrCtrl+Shift+V".to_string(),
            apply_incoming_to_clipboard: true,
            blob_quota_bytes: 2 * 1024 * 1024 * 1024,
            blocked_apps: Vec::new(),
            detect_secrets: true,
            sync_enabled: true,
            announce_on_network: true,
            start_at_login: true,
            device_label: default_device_label(),
        }
    }
}

fn default_device_label() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "This device".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_roundtrip_through_messagepack() {
        let frame = Frame::request(7, Request::History(HistoryQuery::page(50)));
        let bytes = rmp_serde::to_vec_named(&frame).unwrap();
        let back: Frame = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(back.id, 7);
        assert!(matches!(back.body, FrameBody::Request(Request::History(q)) if q.limit == 50));
    }

    #[test]
    fn events_are_unsolicited() {
        assert_eq!(Frame::event(Event::ClipRemoved(ClipId::generate())).id, 0);
    }

    #[test]
    fn default_settings_are_safe() {
        let s = Settings::default();
        assert!(
            s.detect_secrets,
            "secret detection must never default to off"
        );
        assert_eq!(s.blob_quota_bytes, 2 * 1024 * 1024 * 1024);
    }
}
