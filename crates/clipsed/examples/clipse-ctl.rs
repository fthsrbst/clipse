//! A small client for talking to a running `clipsed` from a shell.
//!
//! Exists for the manual verification pass in `docs/manual-verification.md`:
//! checking "did that copy actually reach the history" by looking at a window
//! proves the window works, not the daemon. This asks the daemon directly,
//! over the same protocol the UI uses.
//!
//! ```text
//! cargo run -p clipsed --example clipse-ctl -- --data-dir ./.clipse-dev/a status
//! cargo run -p clipsed --example clipse-ctl -- --data-dir ./.clipse-dev/a history 10
//! cargo run -p clipsed --example clipse-ctl -- --data-dir ./.clipse-dev/a search foo
//! cargo run -p clipsed --example clipse-ctl -- --data-dir ./.clipse-dev/a watch
//! ```

use std::path::PathBuf;

use clipse_core::Paths;
use clipse_ipc::Client;
use clipse_ipc::protocol::{HistoryQuery, Request, Response};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut data_dir: Option<PathBuf> = std::env::var_os("CLIPSE_DATA_DIR").map(PathBuf::from);
    let mut positional: Vec<String> = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => data_dir = args.next().map(PathBuf::from),
            _ => positional.push(arg),
        }
    }

    let paths = match data_dir {
        Some(dir) => Paths::with_root(dir),
        None => Paths::platform_default()?,
    };
    let endpoint = paths.ipc_endpoint();

    let mut client = Client::connect(&endpoint, "clipse-ctl").await?;
    let command = positional.first().map(String::as_str).unwrap_or("status");

    match command {
        "status" => match client.call(Request::Status).await? {
            Response::Status(status) => {
                println!(
                    "device        {} ({})",
                    status.device_label,
                    status.device.short()
                );
                println!("version       {}", status.daemon_version);
                println!("paused        {}", status.paused);
                println!("capture       {:?}", status.capture_mode);
                println!("clips         {}", status.clip_count);
                println!(
                    "blobs         {} / {} bytes",
                    status.blob_bytes, status.blob_quota_bytes
                );
                println!(
                    "peers         {}/{}",
                    status.peers_online, status.peers_total
                );
            }
            other => println!("unexpected: {other:?}"),
        },

        "history" => {
            let limit = positional
                .get(1)
                .and_then(|n| n.parse().ok())
                .unwrap_or(20u32);
            print_clips(
                client
                    .call(Request::History(HistoryQuery::page(limit)))
                    .await?,
            );
        }

        "search" => {
            let text = positional[1..].join(" ");
            let request = Request::Search {
                text,
                query: HistoryQuery::page(50),
            };
            print_clips(client.call(request).await?);
        }

        "settings" => match client.call(Request::GetSettings).await? {
            Response::Settings(settings) => println!("{settings:#?}"),
            other => println!("unexpected: {other:?}"),
        },

        "watch" => {
            println!("subscribed; Ctrl+C to stop");
            let mut events = client.subscribe().await?;
            loop {
                match events.next().await {
                    Ok(event) => println!("{event:?}"),
                    Err(e) => {
                        eprintln!("stream ended: {e}");
                        break;
                    }
                }
            }
        }

        other => {
            eprintln!("unknown command {other:?}");
            eprintln!(
                "usage: clipse-ctl [--data-dir DIR] status|history [N]|search TEXT|settings|watch"
            );
            std::process::exit(2);
        }
    }

    Ok(())
}

fn print_clips(response: Response) {
    match response {
        Response::Clips(clips) => {
            if clips.is_empty() {
                println!("(no clips)");
            }
            for clip in clips {
                let formats: Vec<&str> = clip.payloads.iter().map(|p| p.format.label()).collect();
                println!(
                    "{:<8} {:<6} {:>8}B  {}{}",
                    clip.kind.as_str(),
                    clip.source.device_label,
                    clip.total_size(),
                    if clip.pinned { "* " } else { "" },
                    clip.preview.chars().take(70).collect::<String>(),
                );
                println!("         formats: {}", formats.join(", "));
            }
        }
        other => println!("unexpected: {other:?}"),
    }
}
