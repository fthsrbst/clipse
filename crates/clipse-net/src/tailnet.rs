//! Resolving a peer's tailnet address.
//!
//! There is no multicast on a tailnet, so mDNS never finds a remote peer.
//! Pairing records the peer's tailnet DNS name instead, and this module turns
//! that name into an address by asking the local Tailscale client — which
//! already knows, and which is the only component that can answer while the
//! peer is asleep.
//!
//! Tailscale is optional. A machine without it is a LAN-only Clipse, which is
//! a supported configuration and not a degraded one: [`TailnetStatus::query`]
//! reports [`TailnetError::NotInstalled`] and the candidate list simply has no
//! tailnet entry.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum TailnetError {
    #[error("the tailscale command is not installed")]
    NotInstalled,

    #[error("tailscale is installed but not running (backend state: {state})")]
    NotRunning { state: String },

    #[error("could not run tailscale: {0}")]
    Spawn(#[from] std::io::Error),

    #[error("tailscale exited with status {status}")]
    Failed { status: String },

    #[error("could not understand tailscale's output: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("tailscale did not answer within {}s", QUERY_TIMEOUT.as_secs())]
    TimedOut,
}

/// How long `tailscale status` gets before it is abandoned.
///
/// This is a local socket query and should take milliseconds. It is bounded
/// because it is not always local and not always a query: when the Tailscale
/// backend is wedged — reconnecting, or stuck in `NoState` — the CLI can block
/// indefinitely, and this call sits on the daemon's startup path. An unbounded
/// wait there means a third-party service having a bad day stops Clipse from
/// starting at all, and the user is simply told the app is not running.
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailnetPeer {
    pub host_name: String,
    /// Without the trailing dot Tailscale includes.
    pub dns_name: String,
    pub ips: Vec<IpAddr>,
    pub online: bool,
    pub os: String,
}

impl TailnetPeer {
    /// The address to dial. IPv4 first: every tailnet has a 100.64.0.0/10
    /// address, and not every network path carries IPv6 cleanly.
    pub fn preferred_ip(&self) -> Option<IpAddr> {
        self.ips
            .iter()
            .find(|ip| ip.is_ipv4())
            .or_else(|| self.ips.first())
            .copied()
    }

    /// Does this peer answer to `name`? Accepts the bare hostname, the full
    /// MagicDNS name, and either with or without the trailing dot, because all
    /// three are things a user or a stored pairing record might contain.
    pub fn matches(&self, name: &str) -> bool {
        let wanted = name.trim_end_matches('.');
        self.dns_name.eq_ignore_ascii_case(wanted)
            || self.host_name.eq_ignore_ascii_case(wanted)
            || self
                .dns_name
                .split('.')
                .next()
                .is_some_and(|short| short.eq_ignore_ascii_case(wanted))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TailnetStatus {
    pub backend_state: String,
    pub this_device: Option<TailnetPeer>,
    pub peers: Vec<TailnetPeer>,
}

impl TailnetStatus {
    /// True only when the tailnet can actually carry traffic. Tailscale
    /// reports `NoState` before login and `Stopped` when the user turned it
    /// off; in both cases the peer list is stale or empty and must not be
    /// treated as a route.
    pub fn is_running(&self) -> bool {
        self.backend_state == "Running"
    }

    pub fn find(&self, name: &str) -> Option<&TailnetPeer> {
        self.peers.iter().find(|peer| peer.matches(name))
    }

    pub fn parse(json: &str) -> Result<Self, TailnetError> {
        let raw: RawStatus = serde_json::from_str(json)?;
        Ok(Self {
            backend_state: raw.backend_state.unwrap_or_default(),
            this_device: raw.self_node.map(RawPeer::into_peer),
            peers: raw
                .peer
                .unwrap_or_default()
                .into_values()
                .map(RawPeer::into_peer)
                .collect(),
        })
    }

    /// Ask the local Tailscale client. Blocking — call it from
    /// `spawn_blocking`; it shells out and can take a moment on a cold start.
    pub fn query() -> Result<Self, TailnetError> {
        let exe = tailscale_path().ok_or(TailnetError::NotInstalled)?;
        let mut child = Command::new(exe)
            .args(["status", "--json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        // Drained on its own thread. A child whose pipe fills up blocks on the
        // write, so polling for exit without reading would deadlock on exactly
        // the large outputs — a tailnet with many peers — that matter most.
        let mut stdout = child.stdout.take().expect("stdout was piped");
        let reader = std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = std::io::Read::read_to_end(&mut stdout, &mut buffer);
            buffer
        });

        let deadline = Instant::now() + QUERY_TIMEOUT;
        let exit = loop {
            match child.try_wait()? {
                Some(status) => break status,
                None if Instant::now() >= deadline => {
                    // Killed rather than left running: a wedged CLI would
                    // otherwise accumulate one stuck process per query.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(TailnetError::TimedOut);
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        };

        if !exit.success() {
            return Err(TailnetError::Failed {
                status: exit.to_string(),
            });
        }

        let bytes = reader.join().unwrap_or_default();
        let status = Self::parse(&String::from_utf8_lossy(&bytes))?;
        if !status.is_running() {
            return Err(TailnetError::NotRunning {
                state: status.backend_state,
            });
        }
        Ok(status)
    }
}

/// `tailscale` is often not on `PATH` on Windows and macOS, where it ships
/// inside an application bundle, so the well-known install locations are
/// checked too before giving up.
fn tailscale_path() -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        #[cfg(windows)]
        r"C:\Program Files\Tailscale\tailscale.exe",
        #[cfg(target_os = "macos")]
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        #[cfg(unix)]
        "/usr/bin/tailscale",
        #[cfg(unix)]
        "/usr/local/bin/tailscale",
    ];

    for path in CANDIDATES {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    // Fall back to whatever `PATH` resolves; if it is absent the spawn below
    // fails with NotFound, which the caller turns into NotInstalled.
    which_on_path("tailscale")
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exe_name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    std::env::split_paths(&path)
        .map(|dir| dir.join(&exe_name))
        .find(|candidate| candidate.is_file())
}

// --- Tailscale's own JSON shape -------------------------------------------

#[derive(Deserialize)]
struct RawStatus {
    #[serde(rename = "BackendState")]
    backend_state: Option<String>,
    #[serde(rename = "Self")]
    self_node: Option<RawPeer>,
    #[serde(rename = "Peer")]
    peer: Option<BTreeMap<String, RawPeer>>,
}

#[derive(Deserialize)]
struct RawPeer {
    #[serde(rename = "HostName", default)]
    host_name: String,
    #[serde(rename = "DNSName", default)]
    dns_name: String,
    /// Null rather than empty before login, which is why this is an `Option`
    /// and not a `#[serde(default)] Vec`.
    #[serde(rename = "TailscaleIPs")]
    ips: Option<Vec<IpAddr>>,
    #[serde(rename = "Online", default)]
    online: bool,
    #[serde(rename = "OS", default)]
    os: String,
}

impl RawPeer {
    fn into_peer(self) -> TailnetPeer {
        TailnetPeer {
            host_name: self.host_name,
            dns_name: self.dns_name.trim_end_matches('.').to_string(),
            ips: self.ips.unwrap_or_default(),
            online: self.online,
            os: self.os,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    /// Trimmed from a real `tailscale status --json` on a logged-in machine.
    const RUNNING: &str = r#"{
      "Version": "1.96.3",
      "BackendState": "Running",
      "Self": {
        "HostName": "desktop",
        "DNSName": "desktop.tail1234.ts.net.",
        "OS": "windows",
        "TailscaleIPs": ["100.101.102.103", "fd7a:115c:a1e0::1"],
        "Online": true
      },
      "Peer": {
        "nodekey:aaa": {
          "HostName": "laptop",
          "DNSName": "laptop.tail1234.ts.net.",
          "OS": "macOS",
          "TailscaleIPs": ["100.64.0.7", "fd7a:115c:a1e0::7"],
          "Online": true
        },
        "nodekey:bbb": {
          "HostName": "phone",
          "DNSName": "phone.tail1234.ts.net.",
          "OS": "iOS",
          "TailscaleIPs": ["100.64.0.9"],
          "Online": false
        }
      }
    }"#;

    /// The real output from a machine where Tailscale is installed but has not
    /// finished starting: null IP lists and no peers at all.
    const NOT_LOGGED_IN: &str = r#"{
      "Version": "1.96.3-t3ffddb134-g460d8764a",
      "BackendState": "NoState",
      "TailscaleIPs": null,
      "Self": {
        "HostName": "Fatih",
        "DNSName": "",
        "OS": "windows",
        "TailscaleIPs": null,
        "Online": false
      },
      "Health": ["Tailscale is starting. Please wait."]
    }"#;

    #[test]
    fn parses_a_running_tailnet() {
        let status = TailnetStatus::parse(RUNNING).unwrap();
        assert!(status.is_running());
        assert_eq!(status.peers.len(), 2);
        assert_eq!(status.this_device.unwrap().host_name, "desktop");
    }

    #[test]
    fn strips_the_trailing_dot_from_dns_names() {
        let status = TailnetStatus::parse(RUNNING).unwrap();
        let laptop = status.find("laptop").unwrap();
        assert_eq!(laptop.dns_name, "laptop.tail1234.ts.net");
    }

    #[test]
    fn a_peer_can_be_found_by_any_name_a_pairing_record_might_hold() {
        let status = TailnetStatus::parse(RUNNING).unwrap();
        for name in [
            "laptop",
            "LAPTOP",
            "laptop.tail1234.ts.net",
            "laptop.tail1234.ts.net.",
        ] {
            assert!(status.find(name).is_some(), "{name} did not resolve");
        }
        assert!(status.find("desktop").is_none(), "Self is not a peer");
        assert!(status.find("nonexistent").is_none());
    }

    #[test]
    fn ipv4_is_preferred_but_ipv6_still_works() {
        let status = TailnetStatus::parse(RUNNING).unwrap();
        let laptop = status.find("laptop").unwrap();
        assert_eq!(
            laptop.preferred_ip(),
            Some(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 7)))
        );

        let v6_only = TailnetPeer {
            host_name: "v6".into(),
            dns_name: "v6".into(),
            ips: vec!["fd7a:115c:a1e0::9".parse().unwrap()],
            online: true,
            os: "linux".into(),
        };
        assert!(v6_only.preferred_ip().unwrap().is_ipv6());
    }

    #[test]
    fn an_offline_peer_is_still_listed_with_its_address() {
        // Offline is a routing hint, not a reason to forget where it lives:
        // the peer may wake up before we finish dialling.
        let status = TailnetStatus::parse(RUNNING).unwrap();
        let phone = status.find("phone").unwrap();
        assert!(!phone.online);
        assert!(phone.preferred_ip().is_some());
    }

    #[test]
    fn a_tailscale_that_has_not_logged_in_is_not_running() {
        let status = TailnetStatus::parse(NOT_LOGGED_IN).unwrap();
        assert!(!status.is_running(), "NoState must not count as a route");
        assert_eq!(status.backend_state, "NoState");
        assert!(status.peers.is_empty());
        // Null IP lists must parse as empty, not blow up.
        assert!(status.this_device.unwrap().ips.is_empty());
    }

    #[test]
    fn unknown_fields_do_not_break_parsing() {
        // Tailscale adds fields between releases; a new one must not stop
        // Clipse from syncing.
        let json = r#"{
          "BackendState": "Running",
          "SomethingNew": {"nested": true},
          "Peer": {"nodekey:x": {
            "HostName": "laptop",
            "DNSName": "laptop.ts.net.",
            "TailscaleIPs": ["100.64.0.1"],
            "Online": true,
            "OS": "linux",
            "FutureField": 42
          }}
        }"#;
        let status = TailnetStatus::parse(json).unwrap();
        assert!(status.find("laptop").is_some());
    }

    #[test]
    fn garbage_is_a_parse_error_not_a_panic() {
        assert!(matches!(
            TailnetStatus::parse("not json at all"),
            Err(TailnetError::Parse(_))
        ));
    }

    #[test]
    fn an_empty_object_parses_to_a_not_running_tailnet() {
        let status = TailnetStatus::parse("{}").unwrap();
        assert!(!status.is_running());
        assert!(status.peers.is_empty());
        assert!(status.this_device.is_none());
    }
}
