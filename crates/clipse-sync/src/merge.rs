//! What happens when a clip arrives from a peer.
//!
//! Content is immutable by construction: changing the bytes changes the
//! `ContentHash`, which changes the identity. So there is never a content
//! conflict to resolve — only metadata (`pinned`, `deleted`), and that is
//! settled by last-writer-wins on the [`Hlc`], whose device-id tie-break makes
//! the order total. Two devices merging the same pair therefore always reach
//! the same answer without talking to each other about it.

use clipse_core::Clip;

/// What the caller should do with an incoming clip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeAction {
    /// We have never seen this content. Store it as-is.
    Insert,
    /// We have it, and the peer's copy is newer. Take their `pinned` and
    /// `deleted` flags; keep our own row identity and payloads.
    UpdateMetadata { pinned: bool, deleted: bool },
    /// We have it and ours is at least as new. Nothing to do.
    Ignore,
    /// The clip does not hash to its own payloads. Refusing this is what stops
    /// a buggy or malicious peer from claiming someone else's identity.
    Reject,
}

/// Decide what to do with `incoming`, given what we already have for the same
/// [`clipse_core::ContentHash`] (or `None` if we have nothing).
pub fn merge(local: Option<&Clip>, incoming: &Clip) -> MergeAction {
    if !incoming.hash_matches() {
        return MergeAction::Reject;
    }

    let Some(local) = local else {
        return MergeAction::Insert;
    };

    debug_assert_eq!(
        local.hash, incoming.hash,
        "callers must look the local clip up by the incoming hash"
    );

    if incoming.hlc > local.hlc {
        MergeAction::UpdateMetadata {
            pinned: incoming.pinned,
            deleted: incoming.deleted,
        }
    } else {
        // Equal HLCs mean the same event reached us twice — nothing to do.
        MergeAction::Ignore
    }
}

#[cfg(test)]
mod tests {
    use clipse_core::{ClipFormat, ClipSource, DeviceId, Hlc, Payload};

    use super::*;

    fn clip_with(hlc: Hlc, pinned: bool, deleted: bool) -> Clip {
        let mut clip = Clip::new(
            vec![Payload::new(ClipFormat::Text, b"shared content".to_vec())],
            ClipSource::new(hlc.device, "peer"),
            hlc,
        );
        clip.pinned = pinned;
        clip.deleted = deleted;
        clip
    }

    fn device_pair() -> (DeviceId, DeviceId) {
        let a = DeviceId::generate();
        let b = DeviceId::generate();
        if a.as_uuid() < b.as_uuid() {
            (a, b)
        } else {
            (b, a)
        }
    }

    #[test]
    fn unknown_content_is_inserted() {
        let incoming = clip_with(Hlc::new(10, 0, DeviceId::generate()), false, false);
        assert_eq!(merge(None, &incoming), MergeAction::Insert);
    }

    #[test]
    fn newer_peer_metadata_wins() {
        let device = DeviceId::generate();
        let local = clip_with(Hlc::new(10, 0, device), false, false);
        let incoming = clip_with(Hlc::new(20, 0, device), true, false);

        assert_eq!(
            merge(Some(&local), &incoming),
            MergeAction::UpdateMetadata {
                pinned: true,
                deleted: false
            }
        );
    }

    #[test]
    fn older_peer_metadata_is_ignored() {
        let device = DeviceId::generate();
        let local = clip_with(Hlc::new(20, 0, device), true, false);
        let incoming = clip_with(Hlc::new(10, 0, device), false, false);

        assert_eq!(merge(Some(&local), &incoming), MergeAction::Ignore);
    }

    #[test]
    fn the_same_event_arriving_twice_is_a_no_op() {
        let hlc = Hlc::new(10, 0, DeviceId::generate());
        let local = clip_with(hlc, false, false);
        let incoming = clip_with(hlc, false, false);
        assert_eq!(merge(Some(&local), &incoming), MergeAction::Ignore);
    }

    #[test]
    fn a_deletion_replicates() {
        let device = DeviceId::generate();
        let local = clip_with(Hlc::new(10, 0, device), false, false);
        let incoming = clip_with(Hlc::new(11, 0, device), false, true);

        assert_eq!(
            merge(Some(&local), &incoming),
            MergeAction::UpdateMetadata {
                pinned: false,
                deleted: true
            }
        );
    }

    #[test]
    fn an_undelete_replicates_too() {
        // Deleting on one device and re-copying on another is a real sequence;
        // the later HLC has to win in both directions or the clip would be
        // permanently stuck deleted.
        let device = DeviceId::generate();
        let local = clip_with(Hlc::new(10, 0, device), false, true);
        let incoming = clip_with(Hlc::new(20, 0, device), false, false);

        assert_eq!(
            merge(Some(&local), &incoming),
            MergeAction::UpdateMetadata {
                pinned: false,
                deleted: false
            }
        );
    }

    #[test]
    fn a_forged_clip_is_rejected_before_anything_else() {
        let mut incoming = clip_with(Hlc::new(99, 0, DeviceId::generate()), false, false);
        incoming.payloads = vec![Payload::new(ClipFormat::Text, b"different bytes".to_vec())];

        assert_eq!(merge(None, &incoming), MergeAction::Reject);

        let local = clip_with(Hlc::new(1, 0, DeviceId::generate()), false, false);
        assert_eq!(
            merge(Some(&local), &incoming),
            MergeAction::Reject,
            "a forged clip must not win on HLC either"
        );
    }

    #[test]
    fn concurrent_edits_converge_on_both_devices() {
        // Same wall clock, same counter, different devices: the device-id
        // tie-break has to make both sides pick the same winner, or the two
        // histories would disagree forever.
        let (lo, hi) = device_pair();
        let from_lo = clip_with(Hlc::new(50, 0, lo), true, false);
        let from_hi = clip_with(Hlc::new(50, 0, hi), false, true);

        // Device A has `lo` locally and receives `hi`.
        let on_a = merge(Some(&from_lo), &from_hi);
        // Device B has `hi` locally and receives `lo`.
        let on_b = merge(Some(&from_hi), &from_lo);

        assert_eq!(
            on_a,
            MergeAction::UpdateMetadata {
                pinned: false,
                deleted: true
            },
            "the higher device id must win"
        );
        assert_eq!(on_b, MergeAction::Ignore);

        // Both devices end up with `hi`'s metadata, which is convergence.
    }
}
