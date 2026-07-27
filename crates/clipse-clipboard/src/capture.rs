use clipse_core::{ClipFormat, ContentHash};

use crate::error::Result;

/// Raw clipboard state as read from the OS, before it becomes a `Clip`.
///
/// Deliberately shaped like the inputs to `clipse_core::Payload::new` (format,
/// bytes) rather than `Payload` itself: this crate never computes per-format
/// digests or picks inline-vs-blob storage — that is `clipse-core`'s job once
/// a `Capture` is accepted and handed to the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capture {
    pub payloads: Vec<(ClipFormat, Vec<u8>)>,
    /// Best-effort owning application. Several platforms cannot reliably name
    /// the process that put content on the clipboard (Windows can via the
    /// clipboard owner window; Wayland compositors generally cannot), so this
    /// is `None` rather than a guess when the platform does not know.
    pub app: Option<String>,
}

impl Capture {
    pub fn total_bytes(&self) -> u64 {
        self.payloads.iter().map(|(_, b)| b.len() as u64).sum()
    }

    /// Canonical content hash across every representation, used by the
    /// own-write loop guard and by dedup against the previous capture.
    ///
    /// Sorted by format label first: the OS enumerates clipboard formats in
    /// whatever order the writer happened to set them, and the hash must not
    /// depend on that order (mirrors `clipse_core::Clip::compute_hash`).
    pub fn content_hash(&self) -> ContentHash {
        hash_payloads(&self.payloads)
    }
}

pub(crate) fn hash_payloads(payloads: &[(ClipFormat, Vec<u8>)]) -> ContentHash {
    let mut sorted: Vec<&(ClipFormat, Vec<u8>)> = payloads.iter().collect();
    sorted.sort_by(|a, b| a.0.label().cmp(b.0.label()));
    let parts: Vec<(&str, &[u8])> = sorted
        .iter()
        .map(|(fmt, bytes)| (fmt.label(), bytes.as_slice()))
        .collect();
    ContentHash::of_parts(&parts)
}

/// Reads the current OS clipboard and writes content back to it.
///
/// Implemented once per platform (see `crate::platform`). A single
/// implementation is shared between `Watcher::write` (which arms the
/// `OwnWriteGuard`) and any other caller that only needs to read.
pub trait Clipboard: Send + Sync {
    fn read(&self) -> Result<Option<Capture>>;
    fn write(&self, payloads: &[(ClipFormat, Vec<u8>)]) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_ignores_enumeration_order() {
        let a = Capture {
            payloads: vec![
                (ClipFormat::Text, b"hi".to_vec()),
                (ClipFormat::Html, b"<b>hi</b>".to_vec()),
            ],
            app: None,
        };
        let b = Capture {
            payloads: vec![
                (ClipFormat::Html, b"<b>hi</b>".to_vec()),
                (ClipFormat::Text, b"hi".to_vec()),
            ],
            app: None,
        };
        assert_eq!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn content_hash_changes_with_content() {
        let a = Capture {
            payloads: vec![(ClipFormat::Text, b"hi".to_vec())],
            app: None,
        };
        let b = Capture {
            payloads: vec![(ClipFormat::Text, b"bye".to_vec())],
            app: None,
        };
        assert_ne!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn total_bytes_sums_every_representation() {
        let c = Capture {
            payloads: vec![
                (ClipFormat::Text, vec![0u8; 3]),
                (ClipFormat::Html, vec![0u8; 5]),
            ],
            app: None,
        };
        assert_eq!(c.total_bytes(), 8);
    }
}
