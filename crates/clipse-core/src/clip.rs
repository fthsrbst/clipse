use std::fmt;

use serde::{Deserialize, Serialize};

use crate::hash::ContentHash;
use crate::hlc::Hlc;
use crate::id::{ClipId, DeviceId};

/// Payloads at or below this size live in the SQLite row; anything larger goes
/// to the content-addressed blob store and is transferred with offer+chunk.
pub const INLINE_MAX_BYTES: u64 = 64 * 1024;

/// One representation of a copy operation.
///
/// A single Ctrl+C in a word processor puts plain text, HTML and RTF on the
/// clipboard at once. Clipse keeps all of them so pasting into a rich target
/// stays rich, and pasting into a terminal stays plain.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ClipFormat {
    Text,
    Html,
    Rtf,
    Png,
    Jpeg,
    Svg,
    /// A list of file paths — synced as a reference, not as file contents.
    FileList,
    Other(String),
}

impl ClipFormat {
    /// Stable identifier used in the DB, on the wire and in the content hash.
    /// Never change these strings without bumping `PROTOCOL_VERSION`.
    pub fn label(&self) -> &str {
        match self {
            Self::Text => "text/plain",
            Self::Html => "text/html",
            Self::Rtf => "application/rtf",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Svg => "image/svg+xml",
            Self::FileList => "application/x-clipse-filelist",
            Self::Other(s) => s,
        }
    }

    pub fn from_label(label: &str) -> Self {
        match label {
            "text/plain" => Self::Text,
            "text/html" => Self::Html,
            "application/rtf" => Self::Rtf,
            "image/png" => Self::Png,
            "image/jpeg" => Self::Jpeg,
            "image/svg+xml" => Self::Svg,
            "application/x-clipse-filelist" => Self::FileList,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn kind(&self) -> ClipKind {
        match self {
            Self::Text => ClipKind::Text,
            Self::Html => ClipKind::Html,
            Self::Rtf => ClipKind::Rtf,
            Self::Png | Self::Jpeg | Self::Svg => ClipKind::Image,
            Self::FileList => ClipKind::Files,
            Self::Other(_) => ClipKind::Other,
        }
    }

    /// Higher wins when picking the clip's headline kind for the UI.
    fn display_rank(&self) -> u8 {
        match self {
            Self::FileList => 5,
            Self::Png | Self::Jpeg | Self::Svg => 4,
            Self::Html => 3,
            Self::Rtf => 2,
            Self::Text => 1,
            Self::Other(_) => 0,
        }
    }
}

impl fmt::Display for ClipFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Coarse category used by the UI filters and the type icons.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    Text,
    Html,
    Rtf,
    Image,
    Files,
    Other,
}

impl ClipKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Html => "html",
            Self::Rtf => "rtf",
            Self::Image => "image",
            Self::Files => "files",
            Self::Other => "other",
        }
    }
}

/// Where the bytes actually live.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum PayloadBody {
    /// Small payload stored directly in the clip row.
    Inline(Vec<u8>),
    /// Large payload in the blob store, keyed by `Payload::digest`. May be
    /// absent locally until an offer/chunk transfer completes.
    Blob,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Payload {
    pub format: ClipFormat,
    /// Digest of this representation's bytes on its own.
    pub digest: ContentHash,
    pub size: u64,
    pub body: PayloadBody,
}

impl Payload {
    /// Build a payload, choosing inline vs blob storage by size.
    pub fn new(format: ClipFormat, bytes: Vec<u8>) -> Self {
        let digest = ContentHash::of(&bytes);
        let size = bytes.len() as u64;
        let body = if size <= INLINE_MAX_BYTES {
            PayloadBody::Inline(bytes)
        } else {
            PayloadBody::Blob
        };
        Self {
            format,
            digest,
            size,
            body,
        }
    }

    /// A payload whose bytes are known to be in (or destined for) the blob
    /// store — used when receiving an offer before the chunks arrive.
    pub fn blob(format: ClipFormat, digest: ContentHash, size: u64) -> Self {
        Self {
            format,
            digest,
            size,
            body: PayloadBody::Blob,
        }
    }

    pub fn inline_bytes(&self) -> Option<&[u8]> {
        match &self.body {
            PayloadBody::Inline(b) => Some(b),
            PayloadBody::Blob => None,
        }
    }

    pub fn is_blob(&self) -> bool {
        matches!(self.body, PayloadBody::Blob)
    }
}

/// Provenance of a clip. `app` is best-effort: several platforms do not tell us
/// reliably which process owns the clipboard.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ClipSource {
    pub device: DeviceId,
    pub device_label: String,
    pub app: Option<String>,
}

impl ClipSource {
    pub fn new(device: DeviceId, device_label: impl Into<String>) -> Self {
        Self {
            device,
            device_label: device_label.into(),
            app: None,
        }
    }

    pub fn with_app(mut self, app: Option<String>) -> Self {
        self.app = app;
        self
    }
}

/// One entry in the history.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    /// Identity of the *content*: two devices copying the same thing produce
    /// the same hash, which is what dedup and the sync loop guard key on.
    pub hash: ContentHash,
    pub kind: ClipKind,
    pub payloads: Vec<Payload>,
    /// Short human-readable line for lists and fuzzy search.
    pub preview: String,
    pub source: ClipSource,
    pub hlc: Hlc,
    pub created_at_ms: u64,
    pub pinned: bool,
    /// Tombstone: deletions must replicate, so rows are marked rather than
    /// dropped until every paired device has seen them.
    pub deleted: bool,
}

impl Clip {
    /// Assemble a clip from freshly captured payloads.
    ///
    /// Panics if `payloads` is empty — a capture with no representation is a
    /// bug in the platform backend, not a runtime condition.
    pub fn new(mut payloads: Vec<Payload>, source: ClipSource, hlc: Hlc) -> Self {
        assert!(!payloads.is_empty(), "clip must have at least one payload");

        // Canonical order: the hash must not depend on the order the platform
        // happened to enumerate formats in.
        payloads.sort_by(|a, b| a.format.label().cmp(b.format.label()));

        let hash = Self::compute_hash(&payloads);
        let kind = payloads
            .iter()
            .max_by_key(|p| p.format.display_rank())
            .map(|p| p.format.kind())
            .unwrap_or(ClipKind::Other);
        let preview = build_preview(&payloads, kind);

        Self {
            id: ClipId::generate(),
            hash,
            kind,
            payloads,
            preview,
            source,
            created_at_ms: hlc.wall_ms,
            hlc,
            pinned: false,
            deleted: false,
        }
    }

    /// Hash over every representation's digest. Cheap: never touches blob bytes.
    pub fn compute_hash(payloads: &[Payload]) -> ContentHash {
        let parts: Vec<(&str, &[u8])> = payloads
            .iter()
            .map(|p| (p.format.label(), p.digest.as_bytes().as_slice()))
            .collect();
        ContentHash::of_parts(&parts)
    }

    /// Re-derive the hash and compare — used when accepting a clip from a peer,
    /// so a malicious or buggy sender cannot claim someone else's identity.
    pub fn hash_matches(&self) -> bool {
        Self::compute_hash(&self.payloads) == self.hash
    }

    pub fn payload(&self, format: &ClipFormat) -> Option<&Payload> {
        self.payloads.iter().find(|p| &p.format == format)
    }

    pub fn text(&self) -> Option<&str> {
        self.payload(&ClipFormat::Text)
            .and_then(|p| p.inline_bytes())
            .and_then(|b| std::str::from_utf8(b).ok())
    }

    /// Total bytes across representations; drives the blob quota accounting.
    pub fn total_size(&self) -> u64 {
        self.payloads.iter().map(|p| p.size).sum()
    }

    /// True when some representation still needs fetching from the peer.
    pub fn is_complete(&self, has_blob: impl Fn(&ContentHash) -> bool) -> bool {
        self.payloads
            .iter()
            .all(|p| !p.is_blob() || has_blob(&p.digest))
    }
}

const PREVIEW_MAX_CHARS: usize = 240;

fn build_preview(payloads: &[Payload], kind: ClipKind) -> String {
    if let Some(text) = payloads
        .iter()
        .find(|p| p.format == ClipFormat::Text)
        .and_then(|p| p.inline_bytes())
        .and_then(|b| std::str::from_utf8(b).ok())
    {
        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let trimmed = if collapsed.is_empty() {
            text.trim()
        } else {
            &collapsed
        };
        return trimmed.chars().take(PREVIEW_MAX_CHARS).collect();
    }

    let bytes: u64 = payloads.iter().map(|p| p.size).sum();
    match kind {
        ClipKind::Image => format!("Image · {}", human_size(bytes)),
        ClipKind::Files => format!("{} file(s)", payloads.len()),
        _ => format!("{} · {}", kind.as_str(), human_size(bytes)),
    }
}

fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    match bytes {
        b if b >= MB => format!("{:.1} MB", b as f64 / MB as f64),
        b if b >= KB => format!("{:.0} KB", b as f64 / KB as f64),
        b => format!("{b} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> ClipSource {
        ClipSource::new(DeviceId::generate(), "test")
    }

    fn hlc() -> Hlc {
        Hlc::new(1_700_000_000_000, 0, DeviceId::generate())
    }

    fn text_clip(s: &str) -> Clip {
        Clip::new(
            vec![Payload::new(ClipFormat::Text, s.into())],
            source(),
            hlc(),
        )
    }

    #[test]
    fn identical_content_hashes_equal_across_devices() {
        let a = text_clip("hello");
        let b = text_clip("hello");
        assert_eq!(a.hash, b.hash, "dedup would fail");
        assert_ne!(a.id, b.id, "each capture is its own entry");
    }

    #[test]
    fn hash_is_independent_of_format_order() {
        let payloads = vec![
            Payload::new(ClipFormat::Text, b"hi".to_vec()),
            Payload::new(ClipFormat::Html, b"<b>hi</b>".to_vec()),
        ];
        let reversed: Vec<_> = payloads.iter().cloned().rev().collect();
        let a = Clip::new(payloads, source(), hlc());
        let b = Clip::new(reversed, source(), hlc());
        assert_eq!(a.hash, b.hash);
    }

    #[test]
    fn extra_representation_changes_identity() {
        let plain = text_clip("hi");
        let rich = Clip::new(
            vec![
                Payload::new(ClipFormat::Text, b"hi".to_vec()),
                Payload::new(ClipFormat::Html, b"<b>hi</b>".to_vec()),
            ],
            source(),
            hlc(),
        );
        assert_ne!(plain.hash, rich.hash);
    }

    #[test]
    fn hash_matches_detects_tampering() {
        let mut clip = text_clip("hi");
        assert!(clip.hash_matches());
        clip.payloads[0] = Payload::new(ClipFormat::Text, b"bye".to_vec());
        assert!(!clip.hash_matches(), "forged clip accepted");
    }

    #[test]
    fn large_payload_goes_to_blob_store() {
        let big = vec![7u8; (INLINE_MAX_BYTES + 1) as usize];
        let p = Payload::new(ClipFormat::Png, big);
        assert!(p.is_blob());
        assert!(p.inline_bytes().is_none());

        let small = Payload::new(ClipFormat::Png, vec![7u8; 10]);
        assert!(!small.is_blob());
    }

    #[test]
    fn kind_prefers_the_richest_representation() {
        let clip = Clip::new(
            vec![
                Payload::new(ClipFormat::Text, b"caption".to_vec()),
                Payload::new(ClipFormat::Png, vec![0u8; 32]),
            ],
            source(),
            hlc(),
        );
        assert_eq!(clip.kind, ClipKind::Image);
    }

    #[test]
    fn preview_collapses_whitespace_and_truncates() {
        let clip = text_clip("  line one\n\n\tline two   ");
        assert_eq!(clip.preview, "line one line two");

        let long = "x".repeat(1000);
        assert_eq!(text_clip(&long).preview.chars().count(), PREVIEW_MAX_CHARS);
    }

    #[test]
    fn preview_falls_back_for_binary_clips() {
        let clip = Clip::new(
            vec![Payload::new(ClipFormat::Png, vec![0u8; 2048])],
            source(),
            hlc(),
        );
        assert!(clip.preview.starts_with("Image · 2 KB"), "{}", clip.preview);
    }

    #[test]
    fn incomplete_when_blob_is_missing() {
        let big = Payload::new(ClipFormat::Png, vec![1u8; (INLINE_MAX_BYTES + 1) as usize]);
        let digest = big.digest;
        let clip = Clip::new(vec![big], source(), hlc());
        assert!(!clip.is_complete(|_| false));
        assert!(clip.is_complete(|h| *h == digest));
    }

    #[test]
    fn format_labels_roundtrip() {
        for f in [
            ClipFormat::Text,
            ClipFormat::Html,
            ClipFormat::Rtf,
            ClipFormat::Png,
            ClipFormat::Jpeg,
            ClipFormat::Svg,
            ClipFormat::FileList,
            ClipFormat::Other("application/x-weird".into()),
        ] {
            assert_eq!(ClipFormat::from_label(f.label()), f);
        }
    }
}
