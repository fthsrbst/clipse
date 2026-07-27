//! Translates between SQLite rows and `clipse_core` domain types.
//!
//! Column parsing (`ContentHash::from_str`, `DeviceId::from_str`, ...) can
//! fail with a `clipse_core::Error`, but rusqlite's row-mapping closures must
//! return `rusqlite::Result`. So every raw row is first pulled out as plain
//! strings/ints (infallible from SQLite's point of view) and only converted
//! to domain types afterwards, outside the closure, where `crate::Error` can
//! be returned directly.

use std::str::FromStr;

use clipse_core::{
    Clip, ClipFormat, ClipId, ClipKind, ClipSource, ContentHash, DeviceId, Hlc, Payload,
    PayloadBody,
};
use rusqlite::Row;

use crate::error::Result;

pub(crate) struct RawClip {
    pub id: String,
    pub hash: String,
    pub kind: String,
    pub preview: String,
    pub source_device: String,
    pub source_device_label: String,
    pub source_app: Option<String>,
    pub hlc_wall_ms: i64,
    pub hlc_counter: i64,
    pub hlc_device: String,
    pub created_at_ms: i64,
    pub pinned: i64,
    pub deleted: i64,
}

/// Column order matched by every `SELECT` in this crate that loads a full
/// clip row — keep it in sync with `CLIP_COLUMNS` in `store.rs`.
impl RawClip {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            hash: row.get(1)?,
            kind: row.get(2)?,
            preview: row.get(3)?,
            source_device: row.get(4)?,
            source_device_label: row.get(5)?,
            source_app: row.get(6)?,
            hlc_wall_ms: row.get(7)?,
            hlc_counter: row.get(8)?,
            hlc_device: row.get(9)?,
            created_at_ms: row.get(10)?,
            pinned: row.get(11)?,
            deleted: row.get(12)?,
        })
    }

    pub fn into_clip(self, payloads: Vec<Payload>) -> Result<Clip> {
        let id = ClipId::from_str(&self.id)?;
        let hash = ContentHash::from_str(&self.hash)?;
        let kind = parse_kind(&self.kind);
        let device = DeviceId::from_str(&self.source_device)?;
        let source = ClipSource::new(device, self.source_device_label).with_app(self.source_app);
        let hlc_device = DeviceId::from_str(&self.hlc_device)?;
        let hlc = Hlc::new(self.hlc_wall_ms as u64, self.hlc_counter as u32, hlc_device);

        Ok(Clip {
            id,
            hash,
            kind,
            payloads,
            preview: self.preview,
            source,
            hlc,
            created_at_ms: self.created_at_ms as u64,
            pinned: self.pinned != 0,
            deleted: self.deleted != 0,
        })
    }
}

pub(crate) struct RawPayload {
    pub format_label: String,
    pub digest: String,
    pub size: i64,
    pub inline_bytes: Option<Vec<u8>>,
}

impl RawPayload {
    pub fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            format_label: row.get(0)?,
            digest: row.get(1)?,
            size: row.get(2)?,
            inline_bytes: row.get(3)?,
        })
    }

    pub fn into_payload(self) -> Result<Payload> {
        let format = ClipFormat::from_label(&self.format_label);
        let digest = ContentHash::from_str(&self.digest)?;
        let body = match self.inline_bytes {
            Some(bytes) => PayloadBody::Inline(bytes),
            None => PayloadBody::Blob,
        };
        Ok(Payload {
            format,
            digest,
            size: self.size as u64,
            body,
        })
    }
}

/// `ClipKind` has no public `FromStr`/parser in `clipse-core` (it only ever
/// flows core -> UI there), so the store keeps its own tiny mirror of
/// `ClipKind::as_str`. Falls back to `Other` for a value this build does not
/// recognize rather than failing the whole read — a row written by a newer
/// minor version with an extra kind should still degrade gracefully.
pub(crate) fn parse_kind(s: &str) -> ClipKind {
    match s {
        "text" => ClipKind::Text,
        "html" => ClipKind::Html,
        "rtf" => ClipKind::Rtf,
        "image" => ClipKind::Image,
        "files" => ClipKind::Files,
        _ => ClipKind::Other,
    }
}
