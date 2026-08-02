//! The pairing ceremony: device A shows six digits, the user types them on
//! device B, and if both sides can prove they know those digits *and* agree on
//! each other's static keys, the two devices trust each other from then on.
//!
//! # Design: the code is a secret, not a label
//!
//! The six digits are never sent. They are a short shared secret, established
//! out of band (eyes and fingers), and they enter the ceremony in two places:
//!
//! 1. As a **lookup tag** — `BLAKE3(label || digits)` truncated — which is how
//!    B picks A out of every Clipse device on the network without a URL, a
//!    hostname or a QR code. The tag is the only thing derived from the code
//!    that ever crosses the wire.
//! 2. As part of the **transcript** both sides hash to produce two
//!    confirmation MACs. The transcript also contains each side's own view of
//!    the exchange: both device ids, both static public keys, and both
//!    contributed nonces.
//!
//! Point 2 is what replaces the old "compare six digits on two screens" step.
//! An attacker sitting between A and B (relaying, substituting its own static
//! key on each link, because it has neither party's private key) makes the two
//! transcripts disagree on at least one public key, so the MACs do not verify
//! and *the devices* reject the pairing. The user does not have to notice
//! anything; there is nothing for them to compare. This is the same shape as
//! Bluetooth's passkey entry: a short secret typed on one side, used to
//! authenticate a key exchange, with the check done by the machines.
//!
//! # Design: what this does not defend against, honestly
//!
//! Six digits is 10^6 possibilities. Two attacks follow, and they are handled
//! differently:
//!
//! * **Online guessing** — someone on the network throwing tags at A hoping to
//!   hit the right one. Bounded by [`MAX_LOOKUP_ATTEMPTS`]: an offer that is
//!   probed with the wrong tag that many times is cancelled, so an attacker
//!   gets a handful of guesses out of a million per code the user displays.
//!   That bound is enforced by the caller (`clipsed`'s `PairingState`), which
//!   is the only thing that can count across connections.
//! * **An on-path attacker who completes a ceremony with B and then brute
//!   forces the code offline** from the MAC B sent, and immediately uses it
//!   against A. Nothing here stops that: with a low-entropy secret, only a
//!   PAKE (SPAKE2 and friends) does, and one is not in this crate. The window
//!   is the three minutes a code is displayed, the attacker must already be
//!   in the path between two of the user's own machines, and each code is
//!   good for exactly one ceremony. `docs/decisions.md` records the trade-off
//!   and the upgrade path.
//!
//! # Design: the commit-reveal nonce exchange
//!
//! A fixes its nonce and publishes only a commitment to it in the offer; the
//! nonce itself is revealed in `PairingConfirm`, after B has already sent its
//! own nonce. This stops an *honest-but-dishonest* initiator from choosing its
//! nonce adaptively once it has seen B's, which would otherwise let it steer
//! the transcript. The commitment is deliberately independent of the static
//! key, so key substitution is caught by the MACs and by nothing else — see
//! `mitm_static_key_substitution_is_rejected` below.
//!
//! # Design: replay and expiry
//!
//! An offer carries `expires_at_ms`, checked when it is read and again when
//! the ceremony completes — a handshake an attacker holds open past the window
//! is rejected at the finish line. Beyond that, replay protection is
//! structural: every step consumes `self`, so the same in-memory ceremony
//! object cannot be fed a second accept or confirm. Rust's ownership makes
//! that a compile error rather than a runtime check.

use std::fmt;
use std::net::SocketAddr;

use clipse_core::DeviceId;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::identity::{DeviceIdentity, DevicePublicKey};
use crate::rotation::PairedDevice;

/// Versions the *pairing ceremony's* wire shape, independent of
/// `clipse_core::PROTOCOL_VERSION` (which versions the clip-sync wire
/// format). The two evolve for different reasons — adding a field to
/// `Push` should not force every paired user to re-pair.
///
/// 2: the six-digit code replaced the `clipse://pair/…` URI, and the human
/// comparison step was replaced by the two confirmation MACs.
pub const PAIRING_PROTOCOL_VERSION: u16 = 2;

/// 3 minutes. Long enough that a user can pick up a second device, unlock it
/// and type six digits without racing the clock; short enough that a code seen
/// over someone's shoulder is worthless well within the same sitting.
pub const PAIRING_OFFER_TTL_SECS: u64 = 180;

/// How many wrong lookup tags an offer tolerates before it is cancelled.
///
/// A legitimate pairing produces exactly one lookup at the device the user is
/// looking at: B computes the tag from the typed digits and only the matching
/// device answers. Anything past a handful is someone guessing, and each guess
/// is worth 1 in 10^6 — so a small bound here is what keeps a six-digit secret
/// meaningful against an attacker who can reach the device.
pub const MAX_LOOKUP_ATTEMPTS: u32 = 8;

const COMMIT_LABEL: &[u8] = b"clipse-pairing-commit-v2";
const TAG_LABEL: &[u8] = b"clipse-pairing-tag-v2";
const MAC_A_LABEL: &[u8] = b"clipse-pairing-mac-initiator-v2";
const MAC_B_LABEL: &[u8] = b"clipse-pairing-mac-responder-v2";

/// Length-prefixed hashing so that, e.g., a short label followed by a long
/// nonce cannot collide with a long label followed by a short nonce —
/// concatenation without prefixes is not injective.
fn transcript_hash(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    *hasher.finalize().as_bytes()
}

/// The six decimal digits the user carries from one screen to the other.
///
/// A secret, and treated like one: it is never serialised, never logged and
/// never sent. What travels is [`PairingCode::tag`], a hash, and the two MACs
/// in which the digits are one hashed input among several.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PairingCode([u8; 6]);

impl PairingCode {
    /// Six uniform digits.
    ///
    /// Rejection sampling rather than `byte % 10`: `256 = 25 * 10 + 6`, so
    /// reducing an unfiltered byte mod 10 makes digits 0-5 land with
    /// probability 26/256 and 6-9 with 25/256 — a real, measurable bias.
    /// Rejecting `>= 250` leaves exactly 250 accepted values, 25 per digit,
    /// so `% 10` on an accepted byte is exactly uniform. A secret that has to
    /// be short should at least be the full length it claims.
    pub fn generate() -> Self {
        Self::sample(&mut rand::rng())
    }

    /// The sampling itself, over any byte source, so the uniformity property
    /// can be tested against a seeded generator instead of the OS one — a
    /// statistical test on real randomness is a test that fails once every few
    /// hundred CI runs for no reason.
    fn sample(rng: &mut impl Rng) -> Self {
        let mut digits = [0u8; 6];
        let mut filled = 0;
        let mut buf = [0u8; 32];
        let mut cursor = buf.len(); // force an initial fill
        while filled < digits.len() {
            if cursor == buf.len() {
                rng.fill_bytes(&mut buf);
                cursor = 0;
            }
            let byte = buf[cursor];
            cursor += 1;
            if byte < 250 {
                digits[filled] = byte % 10;
                filled += 1;
            }
        }
        Self(digits)
    }

    /// Read what the user typed. Spaces and dashes are ignored — people type
    /// `482 913` because that is how the other screen shows it.
    pub fn parse(text: &str) -> Result<Self> {
        let mut digits = [0u8; 6];
        let mut count = 0;
        for ch in text.chars() {
            if ch.is_whitespace() || ch == '-' || ch == '_' {
                continue;
            }
            let Some(value) = ch.to_digit(10) else {
                return Err(Error::MalformedPayload);
            };
            if count == digits.len() {
                return Err(Error::MalformedPayload);
            }
            digits[count] = value as u8;
            count += 1;
        }
        if count != digits.len() {
            return Err(Error::MalformedPayload);
        }
        Ok(Self(digits))
    }

    pub fn digits(&self) -> [u8; 6] {
        self.0
    }

    /// Which offer this code is for. The one value derived from the code that
    /// crosses the wire: it lets B address the right device without knowing
    /// its name, and tells A that whoever is calling has at least seen the
    /// screen. Truncated to 16 bytes because it is an identifier, not a
    /// signature.
    pub fn tag(&self) -> [u8; 16] {
        let full = transcript_hash(&[TAG_LABEL, &self.0]);
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&full[..16]);
        tag
    }

    /// Constant-time comparison against a tag that arrived over the network.
    pub fn matches_tag(&self, candidate: &[u8; 16]) -> bool {
        // A 16-byte prefix of a BLAKE3 hash, compared through `blake3::Hash`'s
        // constant-time `PartialEq` by padding both sides identically. Timing
        // on a tag comparison would leak the code one byte at a time.
        let mut ours = [0u8; 32];
        ours[..16].copy_from_slice(&self.tag());
        let mut theirs = [0u8; 32];
        theirs[..16].copy_from_slice(candidate);
        blake3::Hash::from(ours) == blake3::Hash::from(theirs)
    }
}

/// `Display` groups the digits the way the screen shows them. `Debug` does
/// not print them at all: a code that reaches a log file is a code an
/// attacker with the log can use, and `Debug` is what derives and format
/// strings reach for by accident.
impl fmt::Display for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a}{b}{c} {d}{e}{g}")
    }
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PairingCode(******)")
    }
}

/// The device's OS family, shown alongside the label in pairing UI and
/// carried into the persisted [`PairedDevice`] record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
    Other(String),
}

/// One address a peer can be dialled at. LAN and tailnet addresses live in
/// a single ordered list rather than two separate fields — see
/// `docs/decisions.md` ("One QUIC path, an ordered candidate address list")
/// and `docs/sync-protocol.md` §2: dialling walks this list in order and
/// both kinds travel over the same QUIC stack, so there is no reason for
/// this crate to model them as anything but variants of one type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateAddress {
    Lan(SocketAddr),
    Tailnet(SocketAddr),
}

/// What A hands back to whoever presents the right tag. Everything here is
/// public by design — it is the same material a QR code used to carry, and
/// the MACs (not this payload) are what defend against tampering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingOffer {
    pub version: u16,
    pub device_id: DeviceId,
    pub static_public: DevicePublicKey,
    pub label: String,
    pub platform: Platform,
    pub addresses: Vec<CandidateAddress>,
    #[serde(with = "serde_bytes")]
    commitment: [u8; 32],
    pub expires_at_ms: u64,
}

impl PairingOffer {
    pub fn to_bytes(&self) -> Vec<u8> {
        rmp_serde::to_vec(self).expect("PairingOffer fields always serialise")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let offer: Self = rmp_serde::from_slice(bytes).map_err(|_| Error::MalformedPayload)?;
        if offer.version != PAIRING_PROTOCOL_VERSION {
            return Err(Error::MalformedPayload);
        }
        Ok(offer)
    }
}

/// Device B's reply: its own identity, and its half of the nonce pair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingAccept {
    pub device_id: DeviceId,
    pub static_public: DevicePublicKey,
    pub label: String,
    pub platform: Platform,
    pub addresses: Vec<CandidateAddress>,
    #[serde(with = "serde_bytes")]
    nonce: [u8; 32],
}

impl PairingAccept {
    pub fn to_bytes(&self) -> Vec<u8> {
        rmp_serde::to_vec(self).expect("PairingAccept fields always serialise")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        rmp_serde::from_slice(bytes).map_err(|_| Error::MalformedPayload)
    }
}

/// Device A's reveal of the nonce it committed to, plus its proof that it
/// knows the code and saw B's real static key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingConfirm {
    #[serde(with = "serde_bytes")]
    nonce_a: [u8; 32],
    #[serde(with = "serde_bytes")]
    mac: [u8; 32],
}

impl PairingConfirm {
    pub fn to_bytes(&self) -> Vec<u8> {
        rmp_serde::to_vec(self).expect("PairingConfirm fields always serialise")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        rmp_serde::from_slice(bytes).map_err(|_| Error::MalformedPayload)
    }
}

/// B's proof, sent last. Until this verifies, A has trusted nobody: an
/// attacker who relayed the exchange with a substituted key gets this far and
/// no further.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingFinish {
    #[serde(with = "serde_bytes")]
    mac: [u8; 32],
}

impl PairingFinish {
    pub fn to_bytes(&self) -> Vec<u8> {
        rmp_serde::to_vec(self).expect("PairingFinish fields always serialise")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        rmp_serde::from_slice(bytes).map_err(|_| Error::MalformedPayload)
    }
}

/// One frame of the ceremony, in the order they travel:
///
/// ```text
/// B → A   Lookup { tag }      "is this code yours?"
/// A → B   Offer               A's identity and its commitment
/// B → A   Accept              B's identity and its nonce
/// A → B   Confirm             A's nonce, and A's proof
/// B → A   Finish              B's proof
/// A → B   Done                A has committed the pairing
/// ```
///
/// A device that is not the one holding the code answers `Refused` to the
/// lookup and nothing else — identically to a device whose offer expired, so
/// the reply says nothing about which of the two happened.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairingWire {
    Lookup {
        #[serde(with = "serde_bytes")]
        tag: [u8; 16],
    },
    Offer(Box<PairingOffer>),
    Accept(Box<PairingAccept>),
    Confirm(Box<PairingConfirm>),
    Finish(Box<PairingFinish>),
    Done,
    Refused,
}

impl PairingWire {
    pub fn to_bytes(&self) -> Vec<u8> {
        rmp_serde::to_vec(self).expect("PairingWire fields always serialise")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        rmp_serde::from_slice(bytes).map_err(|_| Error::MalformedPayload)
    }
}

/// Everything both sides hash, in one place so the two derivations cannot
/// drift apart. Each side supplies only what it holds or directly received.
struct Transcript<'a> {
    version: u16,
    initiator: DeviceId,
    initiator_key: &'a DevicePublicKey,
    responder: DeviceId,
    responder_key: &'a DevicePublicKey,
    nonce_a: &'a [u8; 32],
    nonce_b: &'a [u8; 32],
    code: &'a PairingCode,
}

impl Transcript<'_> {
    fn mac(&self, label: &[u8]) -> blake3::Hash {
        let version = self.version.to_le_bytes();
        blake3::Hash::from(transcript_hash(&[
            label,
            &version,
            self.initiator.as_bytes(),
            self.initiator_key.as_bytes(),
            self.responder.as_bytes(),
            self.responder_key.as_bytes(),
            self.nonce_a,
            self.nonce_b,
            &self.code.digits(),
        ]))
    }
}

fn random_nonce() -> [u8; 32] {
    let mut nonce = [0u8; 32];
    rand::rng().fill_bytes(&mut nonce);
    nonce
}

/// Device A's side of an in-progress ceremony. Consuming methods enforce
/// one-shot use: see the module-level "replay and expiry" note.
pub struct PairingInitiator {
    offer: PairingOffer,
    nonce_a: [u8; 32],
    code: PairingCode,
}

impl PairingInitiator {
    /// Creates the offer and the code to put on screen. `now_ms` is
    /// caller-supplied (rather than read from the system clock in here) so the
    /// expiry logic stays a pure function of its inputs and is
    /// deterministically testable.
    pub fn create(
        identity: &DeviceIdentity,
        label: String,
        platform: Platform,
        addresses: Vec<CandidateAddress>,
        now_ms: u64,
    ) -> Self {
        let nonce_a = random_nonce();
        let commitment = transcript_hash(&[COMMIT_LABEL, &nonce_a]);
        let offer = PairingOffer {
            version: PAIRING_PROTOCOL_VERSION,
            device_id: identity.device_id(),
            static_public: identity.public_key(),
            label,
            platform,
            addresses,
            commitment,
            expires_at_ms: now_ms.saturating_add(PAIRING_OFFER_TTL_SECS * 1000),
        };
        Self {
            offer,
            nonce_a,
            code: PairingCode::generate(),
        }
    }

    pub fn offer(&self) -> &PairingOffer {
        &self.offer
    }

    /// The digits to show the user.
    pub fn code(&self) -> &PairingCode {
        &self.code
    }

    /// Is this lookup for us? Constant-time, and deliberately not an error
    /// type: a device that is not the one the user typed for has nothing to
    /// report, it just is not the answer.
    pub fn answers(&self, tag: &[u8; 16]) -> bool {
        self.code.matches_tag(tag)
    }

    /// Consumes the pending offer and B's accept, producing the message to
    /// send back plus the half-finished state that still has to see B's proof.
    /// Nothing is trusted at this point — that is what
    /// [`PairingAwaitingProof`] exists to make impossible to skip.
    pub fn accept(
        self,
        accept: &PairingAccept,
        now_ms: u64,
    ) -> Result<(PairingConfirm, PairingAwaitingProof)> {
        if now_ms > self.offer.expires_at_ms {
            return Err(Error::PairingExpired);
        }
        let transcript = Transcript {
            version: self.offer.version,
            initiator: self.offer.device_id,
            initiator_key: &self.offer.static_public,
            responder: accept.device_id,
            responder_key: &accept.static_public,
            nonce_a: &self.nonce_a,
            nonce_b: &accept.nonce,
            code: &self.code,
        };

        let peer = PairedDevice {
            device_id: accept.device_id,
            static_public: accept.static_public,
            label: accept.label.clone(),
            platform: accept.platform.clone(),
            addresses: accept.addresses.clone(),
            paired_at_ms: now_ms,
        };

        Ok((
            PairingConfirm {
                nonce_a: self.nonce_a,
                mac: *transcript.mac(MAC_A_LABEL).as_bytes(),
            },
            PairingAwaitingProof {
                expected: transcript.mac(MAC_B_LABEL),
                peer,
            },
        ))
    }
}

/// A's ceremony after it has answered, holding the one thing it still needs:
/// B's proof that it derived the same transcript.
pub struct PairingAwaitingProof {
    expected: blake3::Hash,
    peer: PairedDevice,
}

impl PairingAwaitingProof {
    /// The last step. A device comes back only if the MACs agree, which they
    /// cannot if anyone substituted a key or guessed the code wrong.
    pub fn verify(self, finish: &PairingFinish) -> Result<PairedDevice> {
        // `blake3::Hash`'s `PartialEq` is constant-time, which is the reason
        // the MACs are carried as hashes rather than byte arrays.
        if blake3::Hash::from(finish.mac) != self.expected {
            return Err(Error::PairingFailed);
        }
        Ok(self.peer)
    }
}

/// Device B's side of an in-progress ceremony.
pub struct PairingResponder {
    offer: PairingOffer,
    nonce_b: [u8; 32],
    accept: PairingAccept,
    code: PairingCode,
}

impl PairingResponder {
    /// Takes the offer A returned for our tag and produces the accept to send
    /// back. Rejects an offer that is already expired or carries an unknown
    /// version — the version check happens here as well as in
    /// [`PairingOffer::from_bytes`] on purpose: that one is a parser other
    /// callers may use for inspection, this is the ceremony's own gate.
    pub fn from_offer(
        offer: PairingOffer,
        code: PairingCode,
        identity: &DeviceIdentity,
        label: String,
        platform: Platform,
        addresses: Vec<CandidateAddress>,
        now_ms: u64,
    ) -> Result<(Self, PairingAccept)> {
        if offer.version != PAIRING_PROTOCOL_VERSION {
            return Err(Error::MalformedPayload);
        }
        if now_ms > offer.expires_at_ms {
            return Err(Error::PairingExpired);
        }
        let accept = PairingAccept {
            device_id: identity.device_id(),
            static_public: identity.public_key(),
            label,
            platform,
            addresses,
            nonce: random_nonce(),
        };
        Ok((
            Self {
                offer,
                nonce_b: accept.nonce,
                accept: accept.clone(),
                code,
            },
            accept,
        ))
    }

    /// Verifies A's revealed nonce against the commitment, then A's MAC, and
    /// only then produces B's own proof and the record to persist. Expiry is
    /// re-checked here so a handshake an attacker stalls past the window
    /// cannot be resurrected by finally delivering the confirm late.
    pub fn verify(
        self,
        confirm: &PairingConfirm,
        now_ms: u64,
    ) -> Result<(PairingFinish, PairedDevice)> {
        if now_ms > self.offer.expires_at_ms {
            return Err(Error::PairingExpired);
        }
        if transcript_hash(&[COMMIT_LABEL, &confirm.nonce_a]) != self.offer.commitment {
            return Err(Error::PairingFailed);
        }

        let transcript = Transcript {
            version: self.offer.version,
            initiator: self.offer.device_id,
            initiator_key: &self.offer.static_public,
            responder: self.accept.device_id,
            responder_key: &self.accept.static_public,
            nonce_a: &confirm.nonce_a,
            nonce_b: &self.nonce_b,
            code: &self.code,
        };

        if blake3::Hash::from(confirm.mac) != transcript.mac(MAC_A_LABEL) {
            return Err(Error::PairingFailed);
        }

        let paired = PairedDevice {
            device_id: self.offer.device_id,
            static_public: self.offer.static_public,
            label: self.offer.label.clone(),
            platform: self.offer.platform.clone(),
            addresses: self.offer.addresses.clone(),
            paired_at_ms: now_ms,
        };
        Ok((
            PairingFinish {
                mac: *transcript.mac(MAC_B_LABEL).as_bytes(),
            },
            paired,
        ))
    }
}

#[cfg(test)]
mod tests {
    use clipse_core::DeviceId;
    use rand::SeedableRng;

    use super::*;

    fn identity() -> DeviceIdentity {
        DeviceIdentity::generate(DeviceId::generate())
    }

    fn addrs() -> Vec<CandidateAddress> {
        vec![
            CandidateAddress::Lan("192.168.1.20:7700".parse().unwrap()),
            CandidateAddress::Tailnet("100.64.0.5:7700".parse().unwrap()),
        ]
    }

    /// One honest run, end to end, the way `clipsed` drives it.
    fn ceremony(
        a: &DeviceIdentity,
        b: &DeviceIdentity,
        now: u64,
    ) -> Result<(PairedDevice, PairedDevice)> {
        let initiator =
            PairingInitiator::create(a, "A's laptop".into(), Platform::Windows, addrs(), now);
        let code = *initiator.code();

        assert!(
            initiator.answers(&code.tag()),
            "the device showing the code must answer its own tag"
        );

        let offer = PairingOffer::from_bytes(&initiator.offer().to_bytes())?;
        let (responder, accept) = PairingResponder::from_offer(
            offer,
            code,
            b,
            "B's desktop".into(),
            Platform::Linux,
            addrs(),
            now + 5_000,
        )?;

        let (confirm, awaiting) = initiator.accept(&accept, now + 6_000)?;
        let (finish, paired_by_b) = responder.verify(&confirm, now + 7_000)?;
        let paired_by_a = awaiting.verify(&finish)?;
        Ok((paired_by_a, paired_by_b))
    }

    #[test]
    fn a_typed_code_pairs_the_two_devices_with_no_comparison_step() {
        let a = identity();
        let b = identity();
        let (paired_by_a, paired_by_b) = ceremony(&a, &b, 1_000).expect("honest run");

        assert_eq!(paired_by_a.device_id, b.device_id());
        assert_eq!(paired_by_b.device_id, a.device_id());
        assert_eq!(paired_by_a.static_public, b.public_key());
        assert_eq!(paired_by_b.static_public, a.public_key());
        assert_eq!(paired_by_a.addresses, addrs());
    }

    #[test]
    fn the_offer_round_trips_with_lan_and_tailnet_addresses() {
        let a = identity();
        let initiator = PairingInitiator::create(&a, "Desktop".into(), Platform::MacOs, addrs(), 0);
        let parsed = PairingOffer::from_bytes(&initiator.offer().to_bytes()).unwrap();
        assert_eq!(&parsed, initiator.offer());
        assert_eq!(parsed.addresses, addrs());
    }

    #[test]
    fn a_truncated_or_foreign_offer_is_rejected() {
        let a = identity();
        let bytes =
            PairingInitiator::create(&a, "X".into(), Platform::Linux, vec![], 0).offer_bytes();
        assert!(matches!(
            PairingOffer::from_bytes(&bytes[..bytes.len() / 2]),
            Err(Error::MalformedPayload)
        ));
        assert!(matches!(
            PairingOffer::from_bytes(b"not msgpack at all"),
            Err(Error::MalformedPayload)
        ));
    }

    /// The property the whole module exists for. An attacker between A and B
    /// substitutes its own static key on each link (it has neither party's
    /// private key, so it must use its own). Under the old design the two
    /// screens showed different digits and a human had to notice; now the
    /// devices themselves refuse.
    #[test]
    fn mitm_static_key_substitution_is_rejected() {
        let a = identity();
        let b = identity();
        let attacker = identity();

        let initiator = PairingInitiator::create(&a, "A".into(), Platform::Windows, vec![], 0);
        let code = *initiator.code();

        // Relayed to B with the attacker's key in place of A's. The attacker
        // cannot forge the commitment (it does not know A's hidden nonce), so
        // everything else is passed through untouched.
        let mut offer_for_b = PairingOffer::from_bytes(&initiator.offer().to_bytes()).unwrap();
        offer_for_b.static_public = attacker.public_key();

        // The attacker also has to know the code to reach B at all — assume the
        // worst and hand it over, so this test isolates key substitution.
        let (responder, accept_seen_by_attacker) = PairingResponder::from_offer(
            offer_for_b,
            code,
            &b,
            "B".into(),
            Platform::Linux,
            vec![],
            0,
        )
        .unwrap();

        let mut accept_for_a = accept_seen_by_attacker.clone();
        accept_for_a.static_public = attacker.public_key();

        let (confirm, awaiting) = initiator.accept(&accept_for_a, 0).unwrap();

        // B refuses A's proof: the two transcripts disagree on both keys.
        assert!(matches!(
            responder.verify(&confirm, 0),
            Err(Error::PairingFailed)
        ));

        // And even if the attacker forged something B would accept, it cannot
        // produce a `PairingFinish` A will take.
        let forged = PairingFinish { mac: [0x5A; 32] };
        assert!(matches!(
            awaiting.verify(&forged),
            Err(Error::PairingFailed)
        ));
    }

    /// Typing the wrong six digits must fail closed, not pair with whatever
    /// answered.
    #[test]
    fn a_wrong_code_cannot_complete_the_ceremony() {
        let a = identity();
        let b = identity();

        let initiator = PairingInitiator::create(&a, "A".into(), Platform::MacOs, vec![], 0);
        let real = *initiator.code();
        let mut wrong_digits = real.digits();
        wrong_digits[0] = (wrong_digits[0] + 1) % 10;
        let wrong = PairingCode(wrong_digits);

        // The tag would not have matched in the first place...
        assert!(!initiator.answers(&wrong.tag()));

        // ...and if it somehow reached the ceremony, the MAC still refuses.
        let offer = PairingOffer::from_bytes(&initiator.offer().to_bytes()).unwrap();
        let (responder, accept) =
            PairingResponder::from_offer(offer, wrong, &b, "B".into(), Platform::Linux, vec![], 0)
                .unwrap();
        let (confirm, _) = initiator.accept(&accept, 0).unwrap();
        assert!(matches!(
            responder.verify(&confirm, 0),
            Err(Error::PairingFailed)
        ));
    }

    #[test]
    fn tampering_with_the_revealed_nonce_fails_the_commitment_check() {
        let a = identity();
        let b = identity();
        let initiator = PairingInitiator::create(&a, "A".into(), Platform::Windows, vec![], 0);
        let code = *initiator.code();
        let offer = PairingOffer::from_bytes(&initiator.offer().to_bytes()).unwrap();
        let (responder, _accept) =
            PairingResponder::from_offer(offer, code, &b, "B".into(), Platform::Linux, vec![], 0)
                .unwrap();

        let forged = PairingConfirm {
            nonce_a: [0xAA; 32],
            mac: [0u8; 32],
        };
        assert!(matches!(
            responder.verify(&forged, 0),
            Err(Error::PairingFailed)
        ));
    }

    #[test]
    fn an_expired_offer_is_rejected_on_receipt_and_at_the_finish_line() {
        let a = identity();
        let b = identity();
        let past_expiry = PAIRING_OFFER_TTL_SECS * 1000 + 1;

        let initiator = PairingInitiator::create(&a, "A".into(), Platform::Windows, vec![], 0);
        let code = *initiator.code();
        let offer = PairingOffer::from_bytes(&initiator.offer().to_bytes()).unwrap();
        assert!(matches!(
            PairingResponder::from_offer(
                offer.clone(),
                code,
                &b,
                "B".into(),
                Platform::Linux,
                vec![],
                past_expiry,
            ),
            Err(Error::PairingExpired)
        ));

        // Started in time, delivered late: A refuses too.
        let (_responder, accept) = PairingResponder::from_offer(
            offer,
            code,
            &b,
            "B".into(),
            Platform::Linux,
            vec![],
            1_000,
        )
        .unwrap();
        assert!(matches!(
            initiator.accept(&accept, past_expiry),
            Err(Error::PairingExpired)
        ));
    }

    #[test]
    fn a_typed_code_is_read_the_way_it_is_shown() {
        let code = PairingCode::parse("482 913").unwrap();
        assert_eq!(code.digits(), [4, 8, 2, 9, 1, 3]);
        assert_eq!(PairingCode::parse("482913").unwrap(), code);
        assert_eq!(PairingCode::parse("482-913").unwrap(), code);
        assert_eq!(code.to_string(), "482 913");

        assert!(PairingCode::parse("48291").is_err(), "too short");
        assert!(PairingCode::parse("4829134").is_err(), "too long");
        assert!(PairingCode::parse("48291x").is_err(), "not a digit");
    }

    /// The code must not leak through the formatting traits every other type
    /// in the workspace derives.
    #[test]
    fn the_code_never_prints_itself_by_accident() {
        let code = PairingCode([1, 2, 3, 4, 5, 6]);
        assert_eq!(format!("{code:?}"), "PairingCode(******)");
    }

    /// Digit generation must not favour any digit in any position — the
    /// rejection sampling in `generate` is the whole reason a six-digit secret
    /// is worth ~20 bits rather than slightly less.
    #[test]
    fn generated_codes_are_unbiased_across_many_samples() {
        // A named, reproducible generator with a fixed seed, not the OS RNG:
        // a chi-square test on real randomness flakes, and a flaky test about
        // a security property is worse than no test.
        let mut rng = rand::rngs::Xoshiro256PlusPlus::seed_from_u64(0xC11D_5EED);
        const SAMPLES: usize = 30_000;
        let mut counts = [[0u32; 10]; 6];
        for _ in 0..SAMPLES {
            let code = PairingCode::sample(&mut rng);
            for (position, &digit) in code.digits().iter().enumerate() {
                assert!(digit < 10);
                counts[position][digit as usize] += 1;
            }
        }

        let expected = SAMPLES as f64 / 10.0;
        for position_counts in counts {
            // Chi-square goodness-of-fit against a uniform distribution over
            // 10 digits has 9 degrees of freedom; 27.88 is the p = 0.001
            // critical value. A generous threshold on purpose — this test's
            // job is to catch a real bug (e.g. an unfiltered `% 10`), not to
            // flake on ordinary sampling noise.
            let chi_square: f64 = position_counts
                .iter()
                .map(|&count| {
                    let diff = count as f64 - expected;
                    diff * diff / expected
                })
                .sum();
            assert!(
                chi_square < 27.88,
                "digit distribution looks biased: counts={position_counts:?} chi_square={chi_square}"
            );
        }
    }

    /// Two codes generated back to back must not share a tag, or the lookup
    /// would send B to the wrong device.
    #[test]
    fn different_codes_have_different_tags() {
        let one = PairingCode([1, 2, 3, 4, 5, 6]);
        let two = PairingCode([1, 2, 3, 4, 5, 7]);
        assert_ne!(one.tag(), two.tag());
        assert!(one.matches_tag(&one.tag()));
        assert!(!one.matches_tag(&two.tag()));
    }

    impl PairingInitiator {
        /// Test helper: the offer as it would go on the wire.
        fn offer_bytes(&self) -> Vec<u8> {
            self.offer.to_bytes()
        }
    }
}
