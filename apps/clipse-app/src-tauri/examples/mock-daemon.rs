//! A stand-in for `clipsed`, which does not exist yet.
//!
//! Speaks the real `clipse_ipc` protocol — modeled on
//! `crates/clipse-ipc/tests/roundtrip.rs` — over the real platform transport,
//! so the Tauri app can be built and exercised against something before the
//! actual daemon lands. It fabricates a small, fixed history (short text, a
//! long text, an HTML+text pair, and an image) and serves History, Search,
//! GetClip, Apply/Paste, pin/delete (with pushed events), Status,
//! SetPaused, Devices (always empty — pairing is F2), and Get/UpdateSettings.
//!
//! Run it with:
//!
//! ```text
//! CLIPSE_DATA_DIR=./.clipse-dev/mock cargo run -p clipse-app --example mock-daemon
//! ```
//!
//! and point the app at the same directory (see `apps/clipse-app/README.md`).

use std::io::Cursor;
use std::sync::Arc;

use clipse_core::{Clip, ClipFormat, ClipSource, DeviceId, HlcClock, Paths, Payload};
use clipse_ipc::codec::{read_frame, write_frame};
use clipse_ipc::transport::{IpcStream, Listener};
use clipse_ipc::{
    CaptureMode, DaemonStatus, ErrorCode, Event, Frame, FrameBody, IpcError, PeerInfo, Request,
    Response, Settings,
};
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use tokio::sync::{Mutex, broadcast};

const BROADCAST_CAPACITY: usize = 64;

struct Daemon {
    device: DeviceId,
    clock: HlcClock,
    clips: Mutex<Vec<Clip>>,
    settings: Mutex<Settings>,
    paused: Mutex<bool>,
    capture_mode: CaptureMode,
    events: broadcast::Sender<Event>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let manual_push = std::env::args().any(|a| a == "--manual-push");
    let data_dir = std::env::var_os("CLIPSE_DATA_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("clipse-mock"));

    let paths = Paths::with_root(&data_dir);
    paths.create_all()?;
    let endpoint = paths.ipc_endpoint();

    let device = DeviceId::generate();
    let capture_mode = if manual_push {
        CaptureMode::ManualPush {
            reason: "GNOME Wayland has no wlr-data-control-style protocol for background \
                     clipboard monitoring."
                .to_string(),
        }
    } else {
        CaptureMode::Automatic
    };

    let daemon = Arc::new(Daemon {
        device,
        clock: HlcClock::new(device),
        clips: Mutex::new(fabricate_clips(device)),
        settings: Mutex::new(Settings::default()),
        paused: Mutex::new(false),
        capture_mode,
        events: broadcast::channel(BROADCAST_CAPACITY).0,
    });

    println!("mock-daemon: listening on {endpoint}");
    println!("mock-daemon: device {device} ({})", device.short());
    println!("mock-daemon: data dir {}", data_dir.display());

    let mut listener = Listener::bind(&endpoint).await?;
    loop {
        let stream = listener.accept().await?;
        let daemon = daemon.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_connection(stream, daemon).await {
                tracing::debug!("connection ended: {e}");
            }
        });
    }
}

async fn serve_connection(mut stream: IpcStream, daemon: Arc<Daemon>) -> anyhow::Result<()> {
    loop {
        let frame = read_frame(&mut stream).await?;
        let FrameBody::Request(request) = frame.body else {
            continue;
        };

        if matches!(request, Request::Subscribe) {
            write_frame(&mut stream, &Frame::response(frame.id, Response::Ok)).await?;
            let mut rx = daemon.events.subscribe();
            loop {
                match rx.recv().await {
                    Ok(event) => write_frame(&mut stream, &Frame::event(event)).await?,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
        }

        let response = handle(&daemon, request).await;
        write_frame(&mut stream, &Frame::response(frame.id, response)).await?;
    }
}

async fn handle(daemon: &Daemon, request: Request) -> Response {
    match request {
        Request::Hello { ipc_version, .. } => {
            if ipc_version != clipse_ipc::IPC_VERSION {
                return Response::Error(IpcError::new(
                    ErrorCode::VersionMismatch,
                    format!(
                        "client speaks IPC version {ipc_version}, this mock daemon speaks {}",
                        clipse_ipc::IPC_VERSION
                    ),
                ));
            }
            Response::Hello {
                daemon_version: format!("mock-{}", env!("CARGO_PKG_VERSION")),
                ipc_version: clipse_ipc::IPC_VERSION,
                device: daemon.device,
            }
        }

        Request::History(query) => {
            let clips = daemon.clips.lock().await;
            Response::Clips(page(&clips, &query, None))
        }

        Request::Search { text, query } => {
            let clips = daemon.clips.lock().await;
            let needle = text.to_lowercase();
            Response::Clips(page(&clips, &query, Some(&needle)))
        }

        Request::GetClip { id } => {
            let clips = daemon.clips.lock().await;
            Response::Clip(clips.iter().find(|c| c.id == id).cloned().map(Box::new))
        }

        // The mock has no blob store, so only inline payloads can be served.
        // That is enough to drive the detail panel, and a blob-backed clip
        // here exercises the "no preview, here is the size" path the real
        // daemon takes past the cap.
        Request::GetPayload { id, format } => {
            let clips = daemon.clips.lock().await;
            let bytes = clips
                .iter()
                .find(|c| c.id == id)
                .and_then(|c| c.payloads.iter().find(|p| p.format == format))
                .and_then(|p| p.inline_bytes())
                .map(|b| serde_bytes::ByteBuf::from(b.to_vec()));
            Response::PayloadBytes(bytes)
        }

        // A mock has no real clipboard owner to reach; acknowledging is
        // enough for the UI to exercise the round trip.
        Request::Apply { .. } | Request::Paste { .. } => Response::Ok,

        Request::SetPinned { id, pinned } => {
            let mut clips = daemon.clips.lock().await;
            match clips.iter_mut().find(|c| c.id == id) {
                Some(clip) => {
                    clip.pinned = pinned;
                    // Bump the HLC so a re-pin also reads as "just touched" —
                    // real edits always advance the clock, and the mock
                    // should exercise that path too.
                    clip.hlc = daemon.clock.now();
                    let _ = daemon
                        .events
                        .send(Event::ClipUpdated(Box::new(clip.clone())));
                    Response::Ok
                }
                None => not_found("no such clip"),
            }
        }

        Request::Delete { id } => {
            let mut clips = daemon.clips.lock().await;
            let before = clips.len();
            clips.retain(|c| c.id != id);
            if clips.len() == before {
                not_found("no such clip")
            } else {
                let _ = daemon.events.send(Event::ClipRemoved(id));
                Response::Ok
            }
        }

        Request::Status => Response::Status(Box::new(status(daemon).await)),

        Request::SetPaused { paused } => {
            *daemon.paused.lock().await = paused;
            let status = status(daemon).await;
            let _ = daemon
                .events
                .send(Event::StatusChanged(Box::new(status.clone())));
            Response::Status(Box::new(status))
        }

        // Pairing is F2 — always empty here.
        Request::Devices => Response::Devices(Vec::<PeerInfo>::new()),
        Request::ForgetDevice { .. } => Response::Ok,

        // The mock has no crypto and no network, so it cannot run a ceremony.
        // Answering Unsupported is what the real daemon does today too, which
        // means the UI's pairing screen sees the same thing either way.
        Request::BeginPairing
        | Request::PairWithUri { .. }
        | Request::ConfirmPairing { .. }
        | Request::CancelPairing => Response::Error(IpcError::new(
            ErrorCode::Unsupported,
            "the mock daemon does not pair",
        )),

        Request::GetSettings => Response::Settings(Box::new(daemon.settings.lock().await.clone())),

        Request::UpdateSettings(new_settings) => {
            let mut settings = daemon.settings.lock().await;
            *settings = *new_settings;
            Response::Settings(Box::new(settings.clone()))
        }

        Request::Subscribe => unreachable!("handled in serve_connection before dispatch"),
    }
}

fn not_found(message: &str) -> Response {
    Response::Error(IpcError::new(ErrorCode::NotFound, message))
}

async fn status(daemon: &Daemon) -> DaemonStatus {
    let clips = daemon.clips.lock().await;
    let blob_bytes: u64 = clips
        .iter()
        .flat_map(|c| &c.payloads)
        .filter(|p| p.is_blob())
        .map(|p| p.size)
        .sum();
    DaemonStatus {
        device: daemon.device,
        device_label: "Mock daemon".to_string(),
        daemon_version: format!("mock-{}", env!("CARGO_PKG_VERSION")),
        paused: *daemon.paused.lock().await,
        capture_mode: daemon.capture_mode.clone(),
        clip_count: clips.len() as u64,
        blob_bytes,
        blob_quota_bytes: Settings::default().blob_quota_bytes,
        peers_online: 0,
        peers_total: 0,
        // Non-zero so the spine's readout is exercised when developing against
        // the mock; a 0 here would look identical to a broken one.
        secrets_refused: 3,
    }
}

/// Filter (optionally by a lowercased substring over the preview and any text
/// payload — a stand-in for the daemon's real FTS5 index), then paginate
/// newest-first.
fn page(clips: &[Clip], query: &clipse_ipc::HistoryQuery, needle: Option<&str>) -> Vec<Clip> {
    let mut filtered: Vec<&Clip> = clips
        .iter()
        .filter(|c| !c.deleted)
        .filter(|c| !query.pinned_only || c.pinned)
        .filter(|c| query.kind.is_none_or(|k| k == c.kind))
        .filter(|c| match needle {
            None => true,
            Some(n) => {
                c.preview.to_lowercase().contains(n)
                    || c.text().is_some_and(|t| t.to_lowercase().contains(n))
            }
        })
        .collect();
    filtered.sort_by_key(|c| std::cmp::Reverse(c.hlc));

    let offset = query.offset as usize;
    let limit = if query.limit == 0 {
        filtered.len()
    } else {
        query.limit as usize
    };
    filtered
        .into_iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .collect()
}

fn fabricate_clips(device: DeviceId) -> Vec<Clip> {
    let source = ClipSource::new(device, "Mock daemon").with_app(Some("mock-daemon".to_string()));
    let clock = HlcClock::new(device);

    let short_text = Clip::new(
        vec![Payload::new(
            ClipFormat::Text,
            b"https://clipse.dev - serverless clipboard sync".to_vec(),
        )],
        source.clone(),
        clock.now(),
    );

    let long_text_body = LONG_TEXT.to_string().into_bytes();
    let long_text = Clip::new(
        vec![Payload::new(ClipFormat::Text, long_text_body)],
        source.clone(),
        clock.now(),
    );

    let html = b"<p>Meeting notes: ship <b>F1</b> before the offsite.</p>".to_vec();
    let plain = b"Meeting notes: ship F1 before the offsite.".to_vec();
    let rich = Clip::new(
        vec![
            Payload::new(ClipFormat::Html, html),
            Payload::new(ClipFormat::Text, plain),
        ],
        source.clone(),
        clock.now(),
    );

    let mut image = Clip::new(
        vec![Payload::new(ClipFormat::Png, fabricate_png())],
        source,
        clock.now(),
    );
    image.pinned = true;

    vec![short_text, long_text, rich, image]
}

const LONG_TEXT: &str = "\
Clipse keeps every representation of a copy operation, not just the last \
one you happened to paste. A single Ctrl+C in a word processor puts plain \
text, HTML and RTF on the clipboard at once, and pasting into a rich text \
target should stay rich while pasting into a terminal stays plain. \
\n\n\
History is unbounded by design: the store is SQLite with an FTS5 index, so \
searching ten years of clips is a query, not a scroll. Large payloads — \
screenshots, long documents — move to a content-addressed blob store keyed \
by a BLAKE3 digest, with an LRU quota that evicts the oldest unpinned blob \
first. \
\n\n\
None of this touches the network unless a second device is paired: Clipse \
is serverless, so two laptops agree on ordering using a hybrid logical \
clock rather than trusting either one's wall clock.";

/// A small eclipse-shaped PNG: a filled amber disc with a cooler bite taken
/// out of it, so the fabricated image clip actually looks like something
/// instead of a gray rectangle.
fn fabricate_png() -> Vec<u8> {
    const SIZE: u32 = 96;
    let radius = SIZE as f32 / 2.0;
    let bite_offset = radius * 0.55;

    let mut buf = image::RgbaImage::new(SIZE, SIZE);
    for (x, y, pixel) in buf.enumerate_pixels_mut() {
        let (dx, dy) = (x as f32 - radius, y as f32 - radius);
        let in_disc = (dx * dx + dy * dy).sqrt() <= radius;

        let (bx, by) = (dx - bite_offset, dy - bite_offset * 0.3);
        let in_bite = (bx * bx + by * by).sqrt() <= radius * 0.92;

        *pixel = if in_disc && !in_bite {
            let t = (dx + radius) / SIZE as f32;
            let r = lerp(0xFF, 0xB4, t);
            let g = lerp(0xB8, 0x6A, t);
            let b = lerp(0x4A, 0x2E, t);
            image::Rgba([r, g, b, 255])
        } else {
            image::Rgba([0, 0, 0, 0])
        };
    }

    let mut bytes = Vec::new();
    PngEncoder::new(Cursor::new(&mut bytes))
        .write_image(buf.as_raw(), SIZE, SIZE, ExtendedColorType::Rgba8)
        .expect("encode fabricated png");
    bytes
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)) as u8
}
