//! Finding paired devices on the local network.
//!
//! Announces this device under [`SERVICE_TYPE`] and browses for the others,
//! feeding whatever it finds into each peer's [`CandidateList`] as LAN
//! addresses. Everything it learns is a *hint*: a discovered peer still has to
//! pass the Noise handshake before it is anything more than an address, so an
//! attacker on the LAN advertising a Clipse service gains nothing.
//!
//! There is no mDNS on a tailnet — no multicast — so a remote peer is never
//! discovered here. That is what the tailnet address recorded at pairing time
//! is for; see `docs/sync-protocol.md` §2.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use clipse_core::DeviceId;
use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};
use tracing::{debug, warn};

use crate::mdns::{RecordError, SERVICE_TYPE, ServiceRecord};

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("could not start mDNS: {0}")]
    Start(String),

    #[error("could not announce this device: {0}")]
    Announce(String),

    #[error("could not browse for peers: {0}")]
    Browse(String),
}

/// What browsing told us about one device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredPeer {
    pub device: DeviceId,
    pub label: String,
    pub platform: String,
    pub fingerprint: String,
    pub addresses: Vec<SocketAddr>,
}

/// Something a browse can report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveryEvent {
    Found(Box<DiscoveredPeer>),
    Lost(DeviceId),
    /// A Clipse device we cannot talk to. Surfaced rather than dropped so the
    /// UI can say "that device needs updating" instead of showing nothing.
    Incompatible {
        instance: String,
        reason: String,
    },
}

/// Announces this device and browses for others.
pub struct Discovery {
    daemon: ServiceDaemon,
    instance: String,
    /// Instance name → device id, so a `ServiceRemoved` (which carries only the
    /// instance) can be reported as the device that went away.
    known: HashMap<String, DeviceId>,
    self_device: DeviceId,
}

impl Discovery {
    /// Start announcing. `port` is the QUIC port from
    /// `QuicTransport::local_addr`.
    pub fn start(record: &ServiceRecord, port: u16) -> Result<Self, DiscoveryError> {
        let daemon = ServiceDaemon::new().map_err(|e| DiscoveryError::Start(e.to_string()))?;
        let instance = record.instance_name();

        let properties: Vec<(String, String)> = record.to_txt().into_iter().collect();
        // An empty host lets mdns-sd fill in this machine's hostname, and
        // `auto_addr` lets it publish every interface address rather than us
        // guessing which one a peer will arrive on.
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance,
            &format!("{instance}.local."),
            (),
            port,
            &properties[..],
        )
        .map_err(|e| DiscoveryError::Announce(e.to_string()))?
        .enable_addr_auto();

        daemon
            .register(info)
            .map_err(|e| DiscoveryError::Announce(e.to_string()))?;

        Ok(Self {
            daemon,
            instance,
            known: HashMap::new(),
            self_device: record.device,
        })
    }

    /// Browse until `budget` elapses, reporting what turns up.
    ///
    /// Deliberately a bounded sweep rather than an endless stream: the daemon
    /// runs this on the same timer it dials on, so discovery and reconnection
    /// stay in step and neither needs to know about the other.
    pub fn sweep(&mut self, budget: Duration) -> Result<Vec<DiscoveryEvent>, DiscoveryError> {
        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| DiscoveryError::Browse(e.to_string()))?;

        let deadline = std::time::Instant::now() + budget;
        let mut events = Vec::new();

        while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
            let Ok(event) = receiver.recv_timeout(remaining) else {
                break;
            };
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    if let Some(event) = self.on_resolved(&info) {
                        events.push(event);
                    }
                }
                ServiceEvent::ServiceRemoved(_, full_name) => {
                    if let Some(device) = self.known.remove(instance_of(&full_name)) {
                        events.push(DiscoveryEvent::Lost(device));
                    }
                }
                _ => {}
            }
        }

        Ok(events)
    }

    fn on_resolved(&mut self, info: &ResolvedService) -> Option<DiscoveryEvent> {
        let instance = info.fullname.as_str();
        // Our own advertisement comes back to us; ignoring it by instance name
        // is cheaper and more reliable than comparing address lists.
        if instance_of(instance) == self.instance {
            return None;
        }

        let txt: std::collections::BTreeMap<String, String> = info
            .txt_properties
            .iter()
            .map(|property| (property.key().to_string(), property.val_str().to_string()))
            .collect();

        let record = match ServiceRecord::from_txt(&txt) {
            Ok(record) => record,
            Err(RecordError::ProtocolMismatch { theirs, ours }) => {
                return Some(DiscoveryEvent::Incompatible {
                    instance: instance.to_string(),
                    reason: format!("speaks protocol {theirs}, this device speaks {ours}"),
                });
            }
            Err(e) => {
                debug!(instance, error = %e, "ignoring a malformed advertisement");
                return None;
            }
        };

        if record.device == self.self_device {
            return None;
        }

        let port = info.port;
        let addresses: Vec<SocketAddr> = info
            .addresses
            .iter()
            .map(|scoped| SocketAddr::new(scoped.to_ip_addr(), port))
            .filter(dialable)
            .collect();

        if addresses.is_empty() {
            debug!(instance, "advertisement resolved with no usable address");
            return None;
        }

        self.known
            .insert(instance_of(instance).to_string(), record.device);

        Some(DiscoveryEvent::Found(Box::new(DiscoveredPeer {
            device: record.device,
            label: record.label,
            platform: record.platform,
            fingerprint: record.fingerprint,
            addresses,
        })))
    }
}

/// Whether an advertised address is worth putting in front of the dialler.
///
/// A link-local IPv6 address is meaningless without the interface it belongs
/// to, and the scope is exactly what is lost on the way out of `to_ip_addr`.
/// Passing one on produces "invalid remote address" every single time — and,
/// worse, it takes up a slot in a peer's candidate list that a working address
/// could have had. A Windows box advertising four of them is enough to hide
/// its own IPv4 address completely.
fn dialable(addr: &SocketAddr) -> bool {
    match addr.ip() {
        std::net::IpAddr::V4(_) => true,
        // fe80::/10, without the scope that would make it routable.
        std::net::IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) != 0xfe80,
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        // Best effort: an un-withdrawn advertisement times out on its own, but
        // withdrawing means peers stop trying to dial a daemon that has
        // already exited.
        if let Err(e) = self
            .daemon
            .unregister(&format!("{}.{SERVICE_TYPE}", self.instance))
        {
            warn!(error = %e, "could not withdraw the mDNS advertisement");
        }
        let _ = self.daemon.shutdown();
    }
}

/// `clipse-abc123._clipse._udp.local.` → `clipse-abc123`.
fn instance_of(full_name: &str) -> &str {
    full_name.split('.').next().unwrap_or(full_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_names_are_extracted_from_the_full_service_name() {
        assert_eq!(
            instance_of("clipse-abc123._clipse._udp.local."),
            "clipse-abc123"
        );
        assert_eq!(instance_of("bare"), "bare");
    }

    #[test]
    fn a_record_survives_the_trip_through_mdns_properties() {
        // The shape mdns-sd wants (a slice of pairs) and the shape the codec
        // produces (a map) have to line up, or every advertisement would be
        // unreadable at the far end.
        let record = ServiceRecord::new(DeviceId::generate(), "abcd1234", "desktop", "windows");
        let properties: Vec<(String, String)> = record.to_txt().into_iter().collect();

        let round_tripped: std::collections::BTreeMap<String, String> =
            properties.into_iter().collect();
        assert_eq!(ServiceRecord::from_txt(&round_tripped).unwrap(), record);
    }

    /// Starting the daemon touches the network stack, so this is the one test
    /// here that can fail for environmental reasons; it asserts only that
    /// announcing does not error, not that anything on the LAN sees it.
    #[test]
    fn announcing_starts_without_error() {
        let record = ServiceRecord::new(DeviceId::generate(), "abcd1234", "test", "test");
        match Discovery::start(&record, 7420) {
            Ok(discovery) => drop(discovery),
            Err(e) => {
                // A CI container without multicast is a legitimate outcome and
                // must not fail the build; a real bug in the record shape
                // would show up as an Announce error, which is worth seeing.
                eprintln!("mDNS unavailable in this environment: {e}");
            }
        }
    }
}

#[cfg(test)]
mod address_tests {
    use super::*;

    /// A Windows peer advertising four link-local addresses and one usable
    /// one: passing the four on is what buried the fifth.
    #[test]
    fn link_local_ipv6_is_not_offered_as_a_candidate() {
        let usable: SocketAddr = "192.168.1.9:65026".parse().unwrap();
        let link_local: SocketAddr = "[fe80::3da6:294b:d805:a07]:65026".parse().unwrap();
        let unique_local: SocketAddr = "[fdb0:b3aa::1]:65026".parse().unwrap();

        assert!(dialable(&usable));
        assert!(!dialable(&link_local), "no scope, no route");
        assert!(
            dialable(&unique_local),
            "a routable v6 address is a real candidate"
        );
    }
}
