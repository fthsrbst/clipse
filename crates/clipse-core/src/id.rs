use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identity of one installation. Generated once at first run, stored in
/// the config file, and bound to the device's long-term X25519 key by pairing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(Uuid);

impl DeviceId {
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    /// Six characters, enough to disambiguate a handful of paired devices in
    /// the UI without showing a full UUID.
    pub fn short(&self) -> String {
        self.0.simple().to_string()[..6].to_string()
    }
}

impl FromStr for DeviceId {
    type Err = crate::Error;

    fn from_str(s: &str) -> crate::Result<Self> {
        Uuid::parse_str(s)
            .map(Self)
            .map_err(|_| crate::Error::InvalidDeviceId(s.to_string()))
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({})", self.short())
    }
}

/// Identity of a single clip entry. Distinct from [`crate::ContentHash`]: the
/// same content copied twice produces two `ClipId`s but one `ContentHash`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClipId(Uuid);

impl ClipId {
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl FromStr for ClipId {
    type Err = crate::Error;

    fn from_str(s: &str) -> crate::Result<Self> {
        Uuid::parse_str(s)
            .map(Self)
            .map_err(|_| crate::Error::InvalidDeviceId(s.to_string()))
    }
}

impl fmt::Display for ClipId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_ids_are_unique_and_roundtrip() {
        let a = DeviceId::generate();
        let b = DeviceId::generate();
        assert_ne!(a, b);
        assert_eq!(a.to_string().parse::<DeviceId>().unwrap(), a);
        assert_eq!(a.short().len(), 6);
    }

    #[test]
    fn rejects_garbage() {
        assert!("not-a-uuid".parse::<DeviceId>().is_err());
    }
}
