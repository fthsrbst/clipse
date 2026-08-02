//! Diagnostic: can this device actually reach the devices it is paired with?
//!
//! The daemon runs inside the app bundle on a desktop, where its log lines go
//! nowhere a person can read them, so "sync does nothing" gives no way to tell
//! an unreachable peer from a broken session. This binds its own ephemeral
//! port with this device's *real* identity — the far side sees a device it
//! already trusts — and reports, per candidate address, whether the dial and
//! the Noise handshake got through.
//!
//! By default it only connects and hangs up: no history is exchanged, nothing
//! is written. `inventory` goes one step further and runs a real session with
//! an *empty* summary — offering nothing, wanting nothing — which makes the
//! peer list everything it holds. That is the only way to see the far side's
//! history from here, and it answers the question a one-sided store cannot:
//! did our clips actually arrive over there.
//!
//! ```text
//! cargo run -p clipsed --example clipse-probe
//! cargo run -p clipsed --example clipse-probe -- 192.168.1.9:58091
//! cargo run -p clipsed --example clipse-probe -- inventory
//! ```

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use clipse_core::Paths;
use clipse_crypto::{CandidateAddress, DeviceIdentity, Trust};
use clipse_net::QuicTransport;
use clipse_net::candidate::{Candidate, CandidateList};

#[derive(serde::Deserialize)]
struct Stored {
    identity: DeviceIdentity,
    trust: Trust,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let inventory = args.iter().any(|arg| arg == "inventory");
    // Extra addresses to try beyond the ones recorded at pairing time — for
    // checking a port that mDNS is advertising but trust has not caught up to.
    let extra: Vec<SocketAddr> = args.iter().filter_map(|arg| arg.parse().ok()).collect();

    let paths = Paths::platform_default()?;
    let file = paths.root().join("identity.json");
    let stored: Stored = serde_json::from_slice(&std::fs::read(&file)?)?;

    let identity = Arc::new(stored.identity);
    println!("this device   {}", identity.device_id());

    // What discovery actually hands the dial loop. A peer whose daemon has
    // restarted is only reachable through this, because the port is ephemeral
    // and the one recorded at pairing time is stale the moment it restarts.
    if args.iter().any(|arg| arg == "discover") {
        let record = clipse_net::ServiceRecord::new(
            identity.device_id(),
            identity.fingerprint().to_string(),
            "clipse-probe",
            std::env::consts::OS,
        );
        let mut discovery = clipse_net::Discovery::start(&record, 0)?;
        for event in discovery.sweep(std::time::Duration::from_secs(5))? {
            match event {
                clipse_net::DiscoveryEvent::Found(peer) => {
                    println!("\nfound {} ({})", peer.label, peer.device.short());
                    for addr in &peer.addresses {
                        println!("  address     {addr}");
                    }
                }
                other => println!("\n{other:?}"),
            }
        }
        return Ok(());
    }

    let peers: Vec<(clipse_core::DeviceId, String, Vec<CandidateAddress>)> = stored
        .trust
        .peers()
        .map(|peer| (peer.device_id, peer.label.clone(), peer.addresses.clone()))
        .collect();
    let trust = Arc::new(RwLock::new(stored.trust));

    let transport = Arc::new(QuicTransport::bind(
        "0.0.0.0:0".parse::<SocketAddr>()?,
        Arc::clone(&identity),
        Arc::clone(&trust),
    )?);
    println!("probing from  {}", transport.local_addr());

    if peers.is_empty() {
        println!("\nno paired devices");
        return Ok(());
    }

    for (device, label, addresses) in peers {
        let mut candidates: Vec<Candidate> = addresses
            .iter()
            .map(|address| match address {
                CandidateAddress::Lan(addr) => Candidate::lan(*addr),
                CandidateAddress::Tailnet(addr) => Candidate::tailnet(*addr),
            })
            .collect();
        candidates.extend(extra.iter().map(|addr| Candidate::lan(*addr)));

        println!("\n{label} ({})", device.short());
        for candidate in &candidates {
            println!(
                "  candidate   {} ({:?})",
                candidate.addr, candidate.reachability
            );
        }

        match transport
            .dial(device, &CandidateList::new(candidates))
            .await
        {
            Ok(mut link) => {
                println!("  REACHED     {}", link.info().addr);
                if inventory && let Err(e) = list_their_history(&mut link, &identity).await {
                    println!("  SESSION FAILED: {e}");
                }
                link.close("probe");
            }
            Err(e) => println!("  UNREACHABLE {e}"),
        }
    }

    Ok(())
}

/// Run one session as dialler, offering nothing, and print what the peer has.
///
/// Follows `sync::run_session`'s alternation exactly — an empty summary is
/// still a turn, and skipping any step deadlocks the far side rather than
/// failing it.
async fn list_their_history(
    link: &mut clipse_net::PeerLink,
    identity: &DeviceIdentity,
) -> Result<(), Box<dyn std::error::Error>> {
    use clipse_sync::SyncMessage;

    let device = identity.device_id();
    link.send(&SyncMessage::Hello {
        device,
        epoch: link.epoch(),
        protocol: clipse_core::PROTOCOL_VERSION,
        max_hlc: None,
        label: "clipse-probe".into(),
        platform: std::env::consts::OS.into(),
    })
    .await?;

    match link.recv().await? {
        SyncMessage::Hello {
            label,
            platform,
            protocol,
            max_hlc,
            ..
        } => println!(
            "  peer says   {label} ({platform}), protocol {protocol}, newest {}",
            max_hlc.map_or("nothing".to_string(), |hlc| hlc.wall_ms.to_string())
        ),
        other => return Err(format!("expected Hello, got {other:?}").into()),
    }

    // Our turn: we have nothing to offer, but the turn still has to be taken.
    link.send(&SyncMessage::Summary {
        entries: Vec::new(),
        complete: true,
    })
    .await?;
    link.recv().await?; // Want (clips) — necessarily empty
    link.recv().await?; // Want (blobs) — likewise
    link.recv().await?; // Ack

    // Their turn: the whole point.
    let mut entries = Vec::new();
    loop {
        match link.recv().await? {
            SyncMessage::Summary {
                entries: page,
                complete,
            } => {
                entries.extend(page);
                if complete {
                    break;
                }
            }
            other => return Err(format!("expected Summary, got {other:?}").into()),
        }
    }

    println!("  peer holds  {} clips", entries.len());
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.hlc.wall_ms));
    for entry in entries.iter().take(12) {
        println!(
            "    {} {:?}{} {}",
            format_ms(entry.hlc.wall_ms),
            entry.kind,
            if entry.deleted { " (deleted)" } else { "" },
            entry.hash
        );
    }

    // Want nothing, then close the turn cleanly so the peer does not log a
    // broken session.
    link.send(&SyncMessage::Want { hashes: Vec::new() }).await?;
    link.send(&SyncMessage::Want { hashes: Vec::new() }).await?;
    link.send(&SyncMessage::Ack {
        hlc: clipse_core::HlcClock::new(device).now(),
    })
    .await?;

    Ok(())
}

fn format_ms(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let time = secs % 86_400;
    format!(
        "{:02}:{:02}:{:02}Z",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}
