//! Encrypted sessions between two paired devices.
//!
//! Uses `Noise_IK_25519_ChaChaPoly_BLAKE2s` via `snow`. IK is the right
//! pattern here specifically because pairing already gave the initiator the
//! responder's static key: IK lets the initiator send its own identity and
//! application-ready data in the very first message (no separate "who are
//! you" round trip) while still hiding the responder's static key from
//! anyone but the intended initiator. See `docs/sync-protocol.md` §3 for how
//! this fits into the wider connection flow (QUIC transport, then this
//! handshake on the first stream).
//!
//! This module never opens a socket. `clipse-net` owns the QUIC stream; it
//! calls [`HandshakeInitiator::start`]/[`finish`](HandshakeInitiator::finish)
//! or [`HandshakeResponder::accept`]/[`respond`](HandshakeResponder::respond)
//! with bytes it read from or is about to write to that stream.

use clipse_core::DeviceId;
use snow::Builder;

use crate::error::{Error, Result};
use crate::identity::{DeviceIdentity, DevicePublicKey};
use crate::rotation::Trust;

/// Noise's own message-size ceiling (not re-exported by `snow`, so restated
/// here): every handshake and transport message fits in one Noise message,
/// and the spec caps that at 65535 bytes regardless of cipher/DH choice.
const MAX_NOISE_MESSAGE: usize = 65_535;
/// ChaCha20-Poly1305's authentication tag length — every transport
/// ciphertext is exactly this many bytes longer than its plaintext.
const TAG_LEN: usize = 16;

/// Rekey after this many transport messages (either direction, combined)...
///
/// ChaCha20-Poly1305 with Noise's 64-bit nonce counter is safe for far more
/// than this — the number is not a cryptographic limit. It exists to bound
/// the blast radius of an assumption elsewhere turning out to be wrong (a
/// bug in this crate, a laptop that stays asleep-then-connected for weeks):
/// forcing a fresh key derivation periodically means a single compromised
/// key protects at most this much traffic, not the lifetime of the pairing.
pub const REKEY_AFTER_MESSAGES: u64 = 10_000;
/// ...or this many ciphertext bytes, whichever threshold is hit first. 64
/// MiB is generous for clipboard payloads (`INLINE_MAX_BYTES` is 64 KiB;
/// even a large pasted image chunked per `docs/sync-protocol.md` §4 rarely
/// approaches this within `REKEY_AFTER_MESSAGES` messages) but still small
/// next to a modern disk or network budget, so it never fires in ordinary
/// use — only on a connection that is unusually long-lived or unusually
/// busy.
pub const REKEY_AFTER_BYTES: u64 = 64 * 1024 * 1024;

fn noise_params() -> snow::params::NoiseParams {
    "Noise_IK_25519_ChaChaPoly_BLAKE2s"
        .parse()
        .expect("hard-coded Noise pattern string is valid")
}

/// The initiator's half of an in-progress handshake. Holds onto the local
/// static key's borrow for as long as `snow` needs it, then drops it once
/// the builder is consumed inside `start`.
pub struct HandshakeInitiator {
    state: snow::HandshakeState,
    remote_device_id: DeviceId,
    remote_static: DevicePublicKey,
    epoch: u64,
}

impl HandshakeInitiator {
    /// Starts a handshake toward a device that must already be paired: IK
    /// requires the initiator to know the responder's static key before the
    /// first message is even built, so an unpaired `remote_device_id` is
    /// refused here, before any bytes exist to send — there is no handshake
    /// attempt to make in the first place.
    pub fn start(
        local: &DeviceIdentity,
        trust: &Trust,
        remote_device_id: DeviceId,
    ) -> Result<(Self, Vec<u8>)> {
        let peer = trust
            .peer(&remote_device_id)
            .ok_or(Error::NotTrusted)?
            .clone();

        let mut state = Builder::new(noise_params())
            .local_private_key(local.secret_key_bytes())
            .map_err(|_| Error::HandshakeFailed)?
            .remote_public_key(peer.static_public.as_bytes())
            .map_err(|_| Error::HandshakeFailed)?
            .build_initiator()
            .map_err(|_| Error::HandshakeFailed)?;

        let mut msg = vec![0u8; MAX_NOISE_MESSAGE];
        let n = state
            .write_message(&[], &mut msg)
            .map_err(|_| Error::HandshakeFailed)?;
        msg.truncate(n);

        Ok((
            Self {
                state,
                remote_device_id,
                remote_static: peer.static_public,
                epoch: trust.epoch(),
            },
            msg,
        ))
    }

    /// Consumes the responder's reply and completes the handshake.
    pub fn finish(mut self, response: &[u8]) -> Result<Session> {
        let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
        self.state
            .read_message(response, &mut buf)
            .map_err(|_| Error::HandshakeFailed)?;
        let transport = self
            .state
            .into_transport_mode()
            .map_err(|_| Error::HandshakeFailed)?;
        Ok(Session::new(
            self.remote_device_id,
            self.remote_static,
            self.epoch,
            transport,
        ))
    }
}

/// The responder's half of an in-progress handshake.
pub struct HandshakeResponder {
    state: snow::HandshakeState,
    remote_device_id: DeviceId,
    remote_static: DevicePublicKey,
    epoch: u64,
}

impl HandshakeResponder {
    /// Reads the initiator's first message and checks its static key against
    /// the trust set. This is the actual security boundary described in
    /// `docs/sync-protocol.md` §3 ("the responder rejects the handshake if
    /// the initiator's static key is not in its paired set"): the check
    /// happens *before* this function returns anything, so an unpaired
    /// peer's handshake dies right here. There is no `respond()` call, no
    /// transport mode, and therefore no code path that could ever decrypt an
    /// application message on its behalf — "must not get as far as
    /// decryption" holds at the level of the encrypted clipboard data, even
    /// though `read_message` on the handshake itself (an inherent part of
    /// IK's design, needed to learn who the initiator even claims to be)
    /// necessarily runs first.
    pub fn accept(local: &DeviceIdentity, trust: &Trust, first_message: &[u8]) -> Result<Self> {
        let mut state = Builder::new(noise_params())
            .local_private_key(local.secret_key_bytes())
            .map_err(|_| Error::HandshakeFailed)?
            .build_responder()
            .map_err(|_| Error::HandshakeFailed)?;

        let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
        state
            .read_message(first_message, &mut buf)
            .map_err(|_| Error::HandshakeFailed)?;

        let remote_static_bytes: [u8; 32] = state
            .get_remote_static()
            .ok_or(Error::HandshakeFailed)?
            .try_into()
            .map_err(|_| Error::HandshakeFailed)?;
        let remote_static = DevicePublicKey::from_bytes(remote_static_bytes);

        let peer = trust.authorize_static_key(&remote_static)?;

        Ok(Self {
            state,
            remote_device_id: peer.device_id,
            remote_static,
            epoch: trust.epoch(),
        })
    }

    /// Produces the reply message and enters transport mode. Only reachable
    /// after `accept` has already authorized the peer.
    pub fn respond(mut self) -> Result<(Session, Vec<u8>)> {
        let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
        let n = self
            .state
            .write_message(&[], &mut buf)
            .map_err(|_| Error::HandshakeFailed)?;
        buf.truncate(n);
        let transport = self
            .state
            .into_transport_mode()
            .map_err(|_| Error::HandshakeFailed)?;
        Ok((
            Session::new(
                self.remote_device_id,
                self.remote_static,
                self.epoch,
                transport,
            ),
            buf,
        ))
    }
}

/// An established, authenticated, encrypted channel to one paired device.
pub struct Session {
    remote_device_id: DeviceId,
    remote_static: DevicePublicKey,
    epoch: u64,
    transport: snow::TransportState,
    messages_since_rekey: u64,
    bytes_since_rekey: u64,
}

impl Session {
    fn new(
        remote_device_id: DeviceId,
        remote_static: DevicePublicKey,
        epoch: u64,
        transport: snow::TransportState,
    ) -> Self {
        Self {
            remote_device_id,
            remote_static,
            epoch,
            transport,
            messages_since_rekey: 0,
            bytes_since_rekey: 0,
        }
    }

    pub fn remote_device_id(&self) -> DeviceId {
        self.remote_device_id
    }

    pub fn remote_static_key(&self) -> DevicePublicKey {
        self.remote_static
    }

    /// The trust epoch active when this session was established. Compared
    /// by [`Trust::authorize_session`] against the trust set's *current*
    /// epoch — see `rotation.rs` for why a mismatch means "reject, force a
    /// re-handshake" rather than anything recoverable in place.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn needs_rekey(&self) -> bool {
        self.messages_since_rekey >= REKEY_AFTER_MESSAGES
            || self.bytes_since_rekey >= REKEY_AFTER_BYTES
    }

    /// Derives fresh sending and receiving keys from the current ones.
    /// Deliberately not triggered automatically inside `write_message` /
    /// `read_message`: both peers must rekey at the same point or one side
    /// decrypts with a key the other has already discarded, and agreeing on
    /// *when* is a transport-level coordination problem (e.g. a control
    /// message on the same stream) that belongs to `clipse-net`, not to this
    /// pure state machine. `needs_rekey` becoming true is the signal; this
    /// method performs only the local half.
    pub fn rekey(&mut self) {
        self.transport.rekey_outgoing();
        self.transport.rekey_incoming();
        self.messages_since_rekey = 0;
        self.bytes_since_rekey = 0;
    }

    /// Encrypts one application message. Refuses once a rekey threshold has
    /// been crossed rather than silently continuing on an over-used key.
    pub fn write_message(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        if self.needs_rekey() {
            return Err(Error::RekeyRequired);
        }
        let mut buf = vec![0u8; plaintext.len() + TAG_LEN];
        let n = self
            .transport
            .write_message(plaintext, &mut buf)
            .map_err(|_| Error::HandshakeFailed)?;
        buf.truncate(n);
        self.messages_since_rekey += 1;
        self.bytes_since_rekey += n as u64;
        Ok(buf)
    }

    /// Decrypts one application message.
    ///
    /// Assumes reliable, in-order delivery. Noise's transport nonce is a
    /// bare monotonic counter with no replay window built in; that is safe
    /// here only because `clipse-net` carries every session on a single
    /// reliable, ordered QUIC stream (`docs/sync-protocol.md` §3) — QUIC's
    /// own stream guarantees are what rule out reordering and duplication,
    /// not anything in this crate. If a future unreliable path (raw UDP,
    /// say, for a tailnet fallback that skips QUIC's stream layer) is ever
    /// added, this assumption stops holding and a real replay/reorder
    /// window would need to sit in front of this method.
    pub fn read_message(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        if self.needs_rekey() {
            return Err(Error::RekeyRequired);
        }
        let mut buf = vec![0u8; ciphertext.len()];
        // A wrong key and a flipped bit both fail Poly1305 verification the
        // same way inside `snow`; both surface as `Error::DecryptFailed`
        // here on purpose (see the coarse-error rationale in `error.rs`).
        let n = self
            .transport
            .read_message(ciphertext, &mut buf)
            .map_err(|_| Error::DecryptFailed)?;
        buf.truncate(n);
        self.messages_since_rekey += 1;
        self.bytes_since_rekey += ciphertext.len() as u64;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use clipse_core::DeviceId;

    use super::*;
    use crate::pairing::{CandidateAddress, Platform};
    use crate::rotation::PairedDevice;

    fn identity() -> DeviceIdentity {
        DeviceIdentity::generate(DeviceId::generate())
    }

    fn record_for(identity: &DeviceIdentity, label: &str) -> PairedDevice {
        PairedDevice {
            device_id: identity.device_id(),
            static_public: identity.public_key(),
            label: label.to_string(),
            platform: Platform::Linux,
            addresses: vec![CandidateAddress::Lan("127.0.0.1:9000".parse().unwrap())],
            paired_at_ms: 0,
        }
    }

    /// Sets up two devices that each trust the other, mirroring what a
    /// completed pairing ceremony would have produced.
    fn paired_pair() -> (DeviceIdentity, Trust, DeviceIdentity, Trust) {
        let alice = identity();
        let bob = identity();
        let mut alice_trust = Trust::new(alice.device_id());
        alice_trust.add_peer(record_for(&bob, "bob"));
        let mut bob_trust = Trust::new(bob.device_id());
        bob_trust.add_peer(record_for(&alice, "alice"));
        (alice, alice_trust, bob, bob_trust)
    }

    fn handshake(
        alice: &DeviceIdentity,
        alice_trust: &Trust,
        bob: &DeviceIdentity,
        bob_trust: &Trust,
    ) -> (Session, Session) {
        let (initiator, msg1) =
            HandshakeInitiator::start(alice, alice_trust, bob.device_id()).unwrap();
        let responder = HandshakeResponder::accept(bob, bob_trust, &msg1).unwrap();
        let (bob_session, msg2) = responder.respond().unwrap();
        let alice_session = initiator.finish(&msg2).unwrap();
        (alice_session, bob_session)
    }

    #[test]
    fn paired_devices_complete_a_handshake_and_exchange_messages() {
        let (alice, alice_trust, bob, bob_trust) = paired_pair();
        let (mut alice_session, mut bob_session) =
            handshake(&alice, &alice_trust, &bob, &bob_trust);

        assert_eq!(alice_session.remote_device_id(), bob.device_id());
        assert_eq!(bob_session.remote_device_id(), alice.device_id());

        let ciphertext = alice_session.write_message(b"copy me").unwrap();
        let plaintext = bob_session.read_message(&ciphertext).unwrap();
        assert_eq!(plaintext, b"copy me");

        let reply = bob_session.write_message(b"got it").unwrap();
        let decrypted_reply = alice_session.read_message(&reply).unwrap();
        assert_eq!(decrypted_reply, b"got it");
    }

    #[test]
    fn tampered_ciphertext_fails_to_decrypt_and_yields_no_plaintext() {
        let (alice, alice_trust, bob, bob_trust) = paired_pair();
        let (mut alice_session, mut bob_session) =
            handshake(&alice, &alice_trust, &bob, &bob_trust);

        let mut ciphertext = alice_session.write_message(b"sensitive clip").unwrap();
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 0x01; // flip one bit of the authentication tag

        let result = bob_session.read_message(&ciphertext);
        assert!(matches!(result, Err(Error::DecryptFailed)));
        // `Result::Err` structurally carries no plaintext — there is no
        // second, separate "did it actually decrypt" flag to get wrong.
    }

    #[test]
    fn unpaired_initiator_is_rejected_before_a_transport_session_exists() {
        let alice = identity();
        let bob = identity();
        let stranger = identity();

        let mut bob_trust = Trust::new(bob.device_id());
        bob_trust.add_peer(record_for(&alice, "alice")); // bob trusts alice, not the stranger

        // The stranger builds a handshake exactly as alice would (it has its
        // own valid keypair — it just isn't in bob's paired set).
        let mut stranger_trust = Trust::new(stranger.device_id());
        stranger_trust.add_peer(record_for(&bob, "bob"));
        let (_initiator, msg1) =
            HandshakeInitiator::start(&stranger, &stranger_trust, bob.device_id()).unwrap();

        let result = HandshakeResponder::accept(&bob, &bob_trust, &msg1);
        assert!(matches!(result, Err(Error::NotTrusted)));
    }

    #[test]
    fn dialling_an_unpaired_device_is_rejected_without_producing_a_message() {
        let alice = identity();
        let stranger_id = DeviceId::generate();
        let alice_trust = Trust::new(alice.device_id()); // empty: nobody paired yet

        let result = HandshakeInitiator::start(&alice, &alice_trust, stranger_id);
        assert!(matches!(result, Err(Error::NotTrusted)));
    }

    #[test]
    fn session_refuses_to_write_past_the_message_threshold_until_rekeyed() {
        let (alice, alice_trust, bob, bob_trust) = paired_pair();
        let (mut alice_session, _bob_session) = handshake(&alice, &alice_trust, &bob, &bob_trust);

        for _ in 0..REKEY_AFTER_MESSAGES {
            alice_session.write_message(b"x").unwrap();
        }
        assert!(alice_session.needs_rekey());
        assert!(matches!(
            alice_session.write_message(b"one too many"),
            Err(Error::RekeyRequired)
        ));

        alice_session.rekey();
        assert!(!alice_session.needs_rekey());
        assert!(alice_session.write_message(b"fresh key").is_ok());
    }
}
