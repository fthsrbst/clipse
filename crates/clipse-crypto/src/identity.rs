//! Long-term device identity.
//!
//! Every device has one X25519 static keypair, generated once and bound to
//! its [`DeviceId`] for as long as the install lives. Pairing exchanges the
//! public half out of band; the private half never leaves this process —
//! it is read from disk, held in memory, and used to authenticate Noise
//! handshakes, and that is all.

use std::fmt;

use clipse_core::DeviceId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use x25519_dalek::StaticSecret;
use zeroize::Zeroizing;

/// An X25519 public key. Not secret — safe to log, display and send in a QR
/// code — so unlike [`DeviceSecretKey`] it gets an ordinary derived `Debug`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DevicePublicKey(#[serde(with = "serde_bytes")] [u8; 32]);

impl DevicePublicKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Human-comparable identity for the "is this really my laptop?" check.
    /// Derived from the public key alone, so both devices in a pairing
    /// compute the identical string independently — nothing needs to be
    /// transmitted for the user to compare it.
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(self)
    }
}

impl fmt::Debug for DevicePublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DevicePublicKey({}…)", &self.to_hex()[..12])
    }
}

impl fmt::Display for DevicePublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// The private half of a device's static keypair.
///
/// Wraps `x25519_dalek::StaticSecret`, which already zeroizes its scalar on
/// drop (the `zeroize` cargo feature is enabled in this crate's
/// `Cargo.toml`). The wrapper's job is the thing `StaticSecret` cannot do on
/// its own: refuse to implement `Debug` in a way that shows the key. A
/// derive here would be silently correct today and a leak the day someone
/// adds `#[derive(Debug)]` to a struct that embeds this one — so the impl
/// below is written by hand and is exactly what the test in this module
/// checks for.
pub struct DeviceSecretKey(StaticSecret);

impl DeviceSecretKey {
    fn generate() -> Self {
        // `getrandom` feature: pulls OS randomness directly rather than
        // routing through a `rand`-crate RNG type, so this crate's two
        // randomness dependencies (x25519-dalek's own CSPRNG use and this
        // crate's `rand` for SAS-adjacent randomness) don't need their
        // `rand_core` trait versions to line up.
        Self(StaticSecret::random())
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    pub(crate) fn public(&self) -> DevicePublicKey {
        DevicePublicKey::from_bytes(*x25519_dalek::PublicKey::from(&self.0).as_bytes())
    }
}

impl fmt::Debug for DeviceSecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceSecretKey(..)")
    }
}

// `StaticSecret`'s own `serde` feature is deliberately left off (see
// Cargo.toml) so we control the wire shape here: a zeroizing byte buffer in,
// a zeroizing byte buffer out, never a `Debug`-printable intermediate.
impl Serialize for DeviceSecretKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serde_bytes::Bytes::new(self.0.as_bytes()).serialize(s)
    }
}

impl<'de> Deserialize<'de> for DeviceSecretKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let bytes = Zeroizing::new(serde_bytes::ByteBuf::deserialize(d)?.into_vec());
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| serde::de::Error::custom("device secret key must be 32 bytes"))?;
        Ok(Self(StaticSecret::from(arr)))
    }
}

/// A device's long-term identity: its stable [`DeviceId`] plus the X25519
/// keypair pairing and sessions authenticate with.
#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceIdentity {
    device_id: DeviceId,
    secret: DeviceSecretKey,
    public: DevicePublicKey,
}

impl DeviceIdentity {
    /// Generate a fresh identity for a device that has none yet. Called
    /// exactly once per install; the result is persisted by the caller
    /// (`clipsed`'s job, not this crate's).
    pub fn generate(device_id: DeviceId) -> Self {
        let secret = DeviceSecretKey::generate();
        let public = secret.public();
        Self {
            device_id,
            secret,
            public,
        }
    }

    pub fn device_id(&self) -> DeviceId {
        self.device_id
    }

    pub fn public_key(&self) -> DevicePublicKey {
        self.public
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.public.fingerprint()
    }

    pub(crate) fn secret_key_bytes(&self) -> &[u8; 32] {
        self.secret.as_bytes()
    }
}

/// Short, human-comparable digest of a public key. Grouped like a product
/// key (`XXXX-XXXX-...`) rather than left as a hex blob because a run of 20
/// undifferentiated hex characters is where visual transposition errors
/// live — humans compare groups, not streams.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Fingerprint(String);

impl Fingerprint {
    fn of(public: &DevicePublicKey) -> Self {
        // Hashed rather than truncated directly from the public key bytes:
        // truncating raw Curve25519 output would leak structure (some bit
        // patterns are more likely than others for a valid point); BLAKE3
        // output is uniform, so any prefix of it is a fair, short digest.
        let digest = blake3::hash(public.as_bytes());
        let hex = hex::encode(&digest.as_bytes()[..10]).to_uppercase();
        let grouped = hex
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).expect("hex is ASCII"))
            .collect::<Vec<_>>()
            .join("-");
        Self(grouped)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_produces_matching_public_key() {
        let id = DeviceIdentity::generate(DeviceId::generate());
        // The public key stored on the struct must be the one that actually
        // corresponds to the secret scalar, not a coincidentally-shaped
        // placeholder — recompute independently and compare.
        let recomputed = x25519_dalek::PublicKey::from(&StaticSecret::from(*id.secret_key_bytes()));
        assert_eq!(id.public_key().as_bytes(), recomputed.as_bytes());
    }

    #[test]
    fn two_identities_never_share_keys() {
        let a = DeviceIdentity::generate(DeviceId::generate());
        let b = DeviceIdentity::generate(DeviceId::generate());
        assert_ne!(a.public_key(), b.public_key());
    }

    #[test]
    fn secret_key_debug_never_reveals_the_scalar() {
        let id = DeviceIdentity::generate(DeviceId::generate());
        let raw_hex = hex::encode(id.secret_key_bytes());
        let debug_of_secret = format!("{:?}", id); // exercises DeviceIdentity's derived Debug too
        assert!(
            !debug_of_secret.contains(&raw_hex),
            "secret bytes leaked through Debug: {debug_of_secret}"
        );
        assert_eq!(
            format!("{:?}", DeviceSecretKey::generate()),
            "DeviceSecretKey(..)"
        );
    }

    #[test]
    fn fingerprint_is_deterministic_and_grouped() {
        let id = DeviceIdentity::generate(DeviceId::generate());
        let a = id.fingerprint();
        let b = id.public_key().fingerprint();
        assert_eq!(a, b);
        assert_eq!(a.as_str().len(), 24, "5 groups of 4 hex chars + 4 dashes");
        assert!(
            a.as_str()
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == '-')
        );
    }

    #[test]
    fn identity_round_trips_through_serde() {
        let id = DeviceIdentity::generate(DeviceId::generate());
        let bytes = rmp_serde::to_vec(&id).unwrap();
        let restored: DeviceIdentity = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(id.device_id(), restored.device_id());
        assert_eq!(id.public_key(), restored.public_key());
        assert_eq!(id.secret_key_bytes(), restored.secret_key_bytes());
    }
}
