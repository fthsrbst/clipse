//! Launching and feeding the macOS notch panel.
//!
//! `apps/clipse-notch` is a separate process. This module starts it, pushes
//! the three most recent clips in as newline-delimited JSON, and reads the
//! actions it writes back out.
//!
//! Keeping the protocol here rather than teaching the sidecar to speak
//! `clipse-ipc` means the wire format exists once. The sidecar knows about
//! three lines of text; it does not know what a `Clip` is, what a `ContentHash`
//! is, or that a daemon exists.
//!
//! Compiled only on macOS. The Swift half has never run — see
//! `docs/roadmap.md` §F3.

#![cfg(target_os = "macos")]

use std::process::Stdio;
use std::sync::Arc;

use clipse_core::Clip;
use serde::Serialize;
use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::state::AppState;

/// How many clips the panel shows. Three is what fits under a notch without
/// the panel becoming a window; the full history is one hotkey away.
const VISIBLE_CLIPS: usize = 3;

#[derive(Serialize)]
struct NotchClip {
    id: String,
    preview: String,
    kind: String,
    source_label: String,
    /// Drives the arrival animation, which is the only moment the user is
    /// told where the thing on their clipboard came from.
    from_peer: bool,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Outgoing<'a> {
    Clips { clips: &'a [NotchClip] },
    Hide,
}

/// A running sidecar, or nothing.
pub struct Notch {
    stdin: Mutex<ChildStdin>,
    _child: Child,
}

impl Notch {
    /// Start the panel. Returns `None` when the binary is missing, which is
    /// the normal case on a machine where it was never bundled — the app must
    /// still run, just without a notch panel.
    pub fn spawn(app: &AppHandle, state: Arc<AppState>, local_device: String) -> Option<Arc<Self>> {
        let path = match tauri::process::current_binary(&app.env()) {
            Ok(exe) => exe.parent()?.join("ClipseNotch"),
            Err(_) => return None,
        };
        if !path.exists() {
            debug!(?path, "no notch sidecar bundled; skipping");
            return None;
        }

        let mut child = Command::new(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| warn!(error = %e, "could not start the notch sidecar"))
            .ok()?;

        let stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;

        let notch = Arc::new(Self {
            stdin: Mutex::new(stdin),
            _child: child,
        });

        // Actions come back as one JSON object per line.
        let action_state = Arc::clone(&state);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(action) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                let (Some(kind), Some(id)) = (
                    action.get("action").and_then(|v| v.as_str()),
                    action.get("clipId").and_then(|v| v.as_str()),
                ) else {
                    continue;
                };
                if kind == "paste"
                    && let Ok(id) = id.parse()
                {
                    let _ = crate::commands::paste_from(&action_state, id).await;
                }
            }
            debug!("notch sidecar closed its output");
        });

        let _ = local_device;
        Some(notch)
    }

    /// Push the current head of the history.
    pub async fn show(&self, clips: &[Clip], local_device: &str) {
        let payload: Vec<NotchClip> = clips
            .iter()
            .filter(|clip| !clip.deleted)
            .take(VISIBLE_CLIPS)
            .map(|clip| NotchClip {
                id: clip.id.to_string(),
                preview: clip.preview.clone(),
                kind: clip.kind.as_str().to_string(),
                source_label: clip.source.device_label.clone(),
                from_peer: clip.source.device.to_string() != local_device,
            })
            .collect();

        self.send(&Outgoing::Clips { clips: &payload }).await;
    }

    pub async fn hide(&self) {
        self.send(&Outgoing::Hide).await;
    }

    async fn send(&self, message: &Outgoing<'_>) {
        let Ok(mut line) = serde_json::to_string(message) else {
            return;
        };
        line.push('\n');

        let mut stdin = self.stdin.lock().await;
        // A sidecar that stopped reading is a dead sidecar; the app carries on
        // without it rather than blocking on a pipe nobody drains.
        if let Err(e) = stdin.write_all(line.as_bytes()).await {
            warn!(error = %e, "notch sidecar is not listening");
            return;
        }
        let _ = stdin.flush().await;
    }
}
