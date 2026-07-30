//! Turns clipboard captures into stored clips.
//!
//! This is the only path from the OS clipboard into the history, which is what
//! makes the privacy guarantee checkable: a capture that `clipse-clipboard`
//! suppressed never reaches this module, so it can reach neither the store nor
//! (in F2) the network.

use std::sync::Arc;

use clipse_clipboard::{CaptureEvent, SuppressionReason};
use clipse_core::{Clip, ClipFormat, ContentHash, INLINE_MAX_BYTES, Payload};
use clipse_ipc::protocol::Event;
use clipse_store::InsertOutcome;
use tokio::sync::mpsc::Receiver;
use tracing::{debug, warn};

use crate::daemon::Daemon;

pub async fn run(daemon: Arc<Daemon>, mut events: Receiver<CaptureEvent>) {
    while let Some(event) = events.recv().await {
        match event {
            CaptureEvent::Suppressed(reason) => {
                // The reason, never the content — this string reaches logs and
                // the UI.
                let label = describe(&reason);
                debug!(reason = %label, "capture suppressed");
                daemon.note_suppression();
                daemon.emit(Event::Suppressed { reason: label });
            }
            CaptureEvent::Captured(capture) => {
                if daemon.is_paused() {
                    debug!("paused; capture dropped");
                    continue;
                }
                if let Err(e) = store_capture(&daemon, capture).await {
                    // Never include the capture in the message: this is the
                    // one place holding clipboard content, and an error path
                    // is exactly where content tends to leak into a log.
                    warn!(error = %e, "could not store a capture");
                }
            }
        }
    }
    debug!("clipboard watcher closed its channel; capture loop ending");
}

async fn store_capture(
    daemon: &Arc<Daemon>,
    capture: clipse_clipboard::Capture,
) -> anyhow::Result<()> {
    let store = daemon.store();
    let hlc = daemon.clock().now();
    let source = daemon.clip_source(capture.app.clone());

    // Anything over the inline limit is written to the blob store first, then
    // referenced by digest — `Payload::new` deliberately drops the bytes for
    // oversized payloads rather than carrying them into the row.
    let mut spill: Vec<(ClipFormat, Vec<u8>)> = Vec::new();
    let mut payloads: Vec<Payload> = Vec::with_capacity(capture.payloads.len());
    for (format, bytes) in capture.payloads {
        if bytes.len() as u64 > INLINE_MAX_BYTES {
            let digest = ContentHash::of(&bytes);
            let size = bytes.len() as u64;
            spill.push((format.clone(), bytes));
            payloads.push(Payload::blob(format, digest, size));
        } else {
            payloads.push(Payload::new(format, bytes));
        }
    }

    let clip = Clip::new(payloads, source, hlc);
    let clip_for_task = clip.clone();
    let store_for_task = Arc::clone(&store);

    let outcome = tokio::task::spawn_blocking(move || -> clipse_store::Result<InsertOutcome> {
        for (_, bytes) in &spill {
            store_for_task.put_blob(&ContentHash::of(bytes), bytes)?;
        }
        let outcome = store_for_task.insert(&clip_for_task)?;
        // Enforced right after a write rather than on a timer: the only thing
        // that can push the blob store over its quota is a write, and the user
        // should not be able to fill a disk between two ticks of a timer.
        store_for_task.enforce_blob_quota()?;
        Ok(outcome)
    })
    .await??;

    match outcome {
        InsertOutcome::Inserted(_) => daemon.emit(Event::ClipAdded(Box::new(clip))),
        InsertOutcome::Deduplicated(id) => {
            // The existing row moved to the top of the history, so the UI has
            // to re-render it — but with the row it already knows, not the
            // duplicate we just built.
            if let Some(existing) = daemon.load_clip(id).await? {
                daemon.emit(Event::ClipUpdated(Box::new(existing)));
            }
        }
        InsertOutcome::Rejected => {
            warn!("store rejected a locally captured clip as hash-mismatched");
        }
    }

    daemon.emit_status();
    Ok(())
}

/// Human-readable, content-free description of a suppression.
fn describe(reason: &SuppressionReason) -> String {
    match reason {
        SuppressionReason::ConcealedFormat => {
            "a password manager marked this copy as private".to_string()
        }
        SuppressionReason::BlockedApp(app) => format!("{app} is on the blocked-app list"),
        SuppressionReason::DetectedSecret(kind) => {
            format!("this looked like a secret ({kind:?})")
        }
        SuppressionReason::Empty => "the clipboard was empty".to_string(),
        SuppressionReason::TooLarge { bytes } => {
            format!("too large to store ({bytes} bytes)")
        }
        SuppressionReason::OwnWrite => "Clipse wrote this itself".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppression_descriptions_never_carry_content() {
        // The only reason that embeds a caller-supplied string is BlockedApp,
        // and an application name is not clipboard content.
        let described = [
            describe(&SuppressionReason::ConcealedFormat),
            describe(&SuppressionReason::Empty),
            describe(&SuppressionReason::OwnWrite),
            describe(&SuppressionReason::TooLarge { bytes: 12 }),
            describe(&SuppressionReason::BlockedApp("1Password".into())),
        ];
        for text in described {
            assert!(!text.is_empty());
        }
    }
}
