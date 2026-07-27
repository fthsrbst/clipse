//! What Clipse advertises on the local network, and what it reads back.
//!
//! Only the record codec lives here — encoding and parsing are the parts worth
//! testing, and they can be tested without a network. The browse/announce
//! wrapper around `mdns-sd` sits on top of this and arrives with the QUIC
//! transport.
//!
//! The advertised record deliberately carries no public key: `fp` is enough
//! for a device that has already paired to recognise a known peer, and tells
//! an unpaired listener nothing it can use.

use std::collections::BTreeMap;

use clipse_core::DeviceId;

pub const SERVICE_TYPE: &str = "_clipse._udp.local.";

/// A single TXT string may not exceed 255 bytes, and the whole record should
/// stay inside one UDP packet. Only the label is user-controlled, so it is the
/// only thing that needs bounding.
const MAX_LABEL_BYTES: usize = 63;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RecordError {
    #[error("advertisement is missing the {0} field")]
    Missing(&'static str),

    #[error("advertisement has an unreadable {field}")]
    Malformed { field: &'static str },

    #[error("advertisement speaks protocol {theirs}, we speak {ours}")]
    ProtocolMismatch { theirs: u16, ours: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceRecord {
    pub protocol: u16,
    pub device: DeviceId,
    /// First 8 hex characters of the device's static-key fingerprint.
    pub fingerprint: String,
    pub label: String,
    pub platform: String,
}

impl ServiceRecord {
    pub fn new(
        device: DeviceId,
        fingerprint: impl Into<String>,
        label: impl Into<String>,
        platform: impl Into<String>,
    ) -> Self {
        Self {
            protocol: clipse_core::PROTOCOL_VERSION,
            device,
            fingerprint: fingerprint.into(),
            label: truncate_on_char_boundary(&label.into(), MAX_LABEL_BYTES),
            platform: platform.into(),
        }
    }

    /// The mDNS instance name. Uses the device id rather than the label so two
    /// machines both called "laptop" do not collide on the network.
    pub fn instance_name(&self) -> String {
        format!("clipse-{}", self.device.short())
    }

    pub fn to_txt(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("v".to_string(), self.protocol.to_string()),
            ("id".to_string(), self.device.to_string()),
            ("fp".to_string(), self.fingerprint.clone()),
            ("label".to_string(), self.label.clone()),
            ("os".to_string(), self.platform.clone()),
        ])
    }

    /// Parse a peer's advertisement.
    ///
    /// A protocol mismatch is rejected here rather than at connect time: there
    /// is no point dialling a daemon we cannot talk to, and the UI can say
    /// "that device needs updating" instead of showing a connection that keeps
    /// failing.
    pub fn from_txt(txt: &BTreeMap<String, String>) -> Result<Self, RecordError> {
        let get = |key: &'static str| txt.get(key).ok_or(RecordError::Missing(key));

        let protocol: u16 = get("v")?
            .parse()
            .map_err(|_| RecordError::Malformed { field: "v" })?;
        if protocol != clipse_core::PROTOCOL_VERSION {
            return Err(RecordError::ProtocolMismatch {
                theirs: protocol,
                ours: clipse_core::PROTOCOL_VERSION,
            });
        }

        let device: DeviceId = get("id")?
            .parse()
            .map_err(|_| RecordError::Malformed { field: "id" })?;

        Ok(Self {
            protocol,
            device,
            fingerprint: get("fp")?.clone(),
            label: get("label")?.clone(),
            platform: get("os")?.clone(),
        })
    }
}

/// Cutting a UTF-8 string at a byte count can split a character in half; this
/// steps back to the nearest boundary rather than producing invalid UTF-8 that
/// a peer would then fail to parse.
fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> ServiceRecord {
        ServiceRecord::new(
            DeviceId::generate(),
            "a1b2c3d4",
            "Fatih's desktop",
            "windows",
        )
    }

    #[test]
    fn a_record_round_trips_through_txt() {
        let original = record();
        let parsed = ServiceRecord::from_txt(&original.to_txt()).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn the_public_key_is_never_advertised() {
        let txt = record().to_txt();
        assert_eq!(
            txt.keys().cloned().collect::<Vec<_>>(),
            vec!["fp", "id", "label", "os", "v"],
            "an extra advertised field is a privacy decision, not a detail"
        );
    }

    #[test]
    fn instance_names_do_not_collide_for_identically_named_devices() {
        let a = ServiceRecord::new(DeviceId::generate(), "aaaa", "laptop", "macos");
        let b = ServiceRecord::new(DeviceId::generate(), "bbbb", "laptop", "macos");
        assert_ne!(a.instance_name(), b.instance_name());
    }

    #[test]
    fn a_peer_on_another_protocol_version_is_rejected_not_dialled() {
        let mut txt = record().to_txt();
        txt.insert("v".into(), "9999".into());

        assert_eq!(
            ServiceRecord::from_txt(&txt),
            Err(RecordError::ProtocolMismatch {
                theirs: 9999,
                ours: clipse_core::PROTOCOL_VERSION,
            })
        );
    }

    #[test]
    fn missing_fields_are_named_in_the_error() {
        for key in ["v", "id", "fp", "label", "os"] {
            let mut txt = record().to_txt();
            txt.remove(key);
            assert_eq!(
                ServiceRecord::from_txt(&txt),
                Err(RecordError::Missing(key))
            );
        }
    }

    #[test]
    fn a_malformed_field_is_reported_not_guessed() {
        let mut txt = record().to_txt();
        txt.insert("id".into(), "not-a-uuid".into());
        assert_eq!(
            ServiceRecord::from_txt(&txt),
            Err(RecordError::Malformed { field: "id" })
        );

        let mut txt = record().to_txt();
        txt.insert("v".into(), "not-a-number".into());
        assert_eq!(
            ServiceRecord::from_txt(&txt),
            Err(RecordError::Malformed { field: "v" })
        );
    }

    #[test]
    fn a_long_label_is_bounded_without_breaking_utf8() {
        let long = "é".repeat(200);
        let record = ServiceRecord::new(DeviceId::generate(), "aaaa", long, "linux");

        assert!(record.label.len() <= MAX_LABEL_BYTES);
        // The real test: it is still valid UTF-8 and still parses back.
        assert!(record.label.chars().all(|c| c == 'é'));
        assert_eq!(ServiceRecord::from_txt(&record.to_txt()).unwrap(), record);
    }

    #[test]
    fn a_short_label_is_left_alone() {
        let record = ServiceRecord::new(DeviceId::generate(), "aaaa", "laptop", "linux");
        assert_eq!(record.label, "laptop");
    }
}
