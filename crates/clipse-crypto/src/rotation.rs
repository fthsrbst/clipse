//! The trust set: the local device's view of which other devices it talks
//! to, and what happens to that view when the user removes one.
//!
//! Removing a device is the one operation in this crate that must have a
//! blast radius bigger than "delete one row": every *other* session this
//! device has open, with every *other* peer, must stop trusting itself the
//! moment one peer is kicked out — otherwise a removed laptop that already
//! captured a session key keeps reading clipboard traffic between the two
//! devices that remain, forever, with no way for the user to know. An
//! `epoch` counter is the cheap mechanism for that: bump it on removal, and
//! every session stamped with an older epoch is rejected outright, forcing a
//! fresh Noise handshake (which itself re-checks the paired set) before any
//! more application data flows. Nothing has to track *which* keys the
//! removed device might have observed — the epoch makes all of them stale at
//! once.

use std::collections::BTreeMap;

use clipse_core::DeviceId;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::identity::DevicePublicKey;
use crate::pairing::{CandidateAddress, Platform};
use crate::session::Session;

/// The output of a completed pairing ceremony, and an entry in [`Trust`]'s
/// paired set. Persisting it (writing it to disk, showing it in the paired
/// devices list) is `clipsed`'s job — this type only needs to be
/// serialisable for that, not to know how it happens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairedDevice {
    pub device_id: DeviceId,
    pub static_public: DevicePublicKey,
    pub label: String,
    pub platform: Platform,
    pub addresses: Vec<CandidateAddress>,
    pub paired_at_ms: u64,
}

/// The local device's identity plus everyone it currently trusts, versioned
/// by an epoch that only moves forward.
#[derive(Debug, Serialize, Deserialize)]
pub struct Trust {
    local_device_id: DeviceId,
    epoch: u64,
    peers: BTreeMap<DeviceId, PairedDevice>,
}

impl Trust {
    pub fn new(local_device_id: DeviceId) -> Self {
        Self {
            local_device_id,
            epoch: 0,
            peers: BTreeMap::new(),
        }
    }

    pub fn local_device_id(&self) -> DeviceId {
        self.local_device_id
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn is_paired(&self, device_id: &DeviceId) -> bool {
        self.peers.contains_key(device_id)
    }

    pub fn peer(&self, device_id: &DeviceId) -> Option<&PairedDevice> {
        self.peers.get(device_id)
    }

    pub fn peer_by_static_key(&self, key: &DevicePublicKey) -> Option<&PairedDevice> {
        self.peers.values().find(|peer| &peer.static_public == key)
    }

    pub fn peers(&self) -> impl Iterator<Item = &PairedDevice> {
        self.peers.values()
    }

    /// Adding a device does not bump the epoch. Nothing is being revoked —
    /// sessions already established with *other* peers under the current
    /// epoch stay valid. Only removal, which changes who is trusted in a way
    /// that could otherwise go unnoticed by an open session, forces
    /// everything to re-handshake.
    pub fn add_peer(&mut self, peer: PairedDevice) {
        self.peers.insert(peer.device_id, peer);
    }

    /// Removes a device and bumps the epoch. From this call onward:
    /// - the removed device's static key no longer authorizes a handshake
    ///   ([`HandshakeResponder::accept`](crate::session::HandshakeResponder::accept)
    ///   looks it up here and will not find it),
    /// - every [`Session`] anyone established while the old epoch was
    ///   current is refused by [`Trust::authorize_session`], including
    ///   sessions with peers who were *not* removed — they simply re-run the
    ///   handshake, which is cheap, rather than this type trying to reason
    ///   about which sessions the removed device could plausibly have
    ///   touched.
    pub fn remove_peer(&mut self, device_id: &DeviceId) -> Result<PairedDevice> {
        let removed = self.peers.remove(device_id).ok_or(Error::NotTrusted)?;
        self.epoch += 1;
        Ok(removed)
    }

    /// The check [`crate::session::HandshakeResponder::accept`] performs
    /// before it will let a handshake proceed toward a transport session.
    pub(crate) fn authorize_static_key(&self, key: &DevicePublicKey) -> Result<&PairedDevice> {
        self.peer_by_static_key(key).ok_or(Error::NotTrusted)
    }

    /// A session is only usable while its epoch still matches. Per
    /// `docs/sync-protocol.md` §3, the wire-level check the responder
    /// performs is against the epoch carried in the application's `Hello`
    /// message (owned by `clipse-net`/`clipse-sync`, not this crate); this
    /// method is the primitive that comparison reduces to once `clipse-net`
    /// has decoded that field.
    pub fn authorize_session(&self, session: &Session) -> Result<()> {
        if session.epoch() != self.epoch {
            return Err(Error::NotTrusted);
        }
        if !self.is_paired(&session.remote_device_id()) {
            return Err(Error::NotTrusted);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use clipse_core::DeviceId;

    use super::*;
    use crate::identity::DeviceIdentity;
    use crate::session::{HandshakeInitiator, HandshakeResponder};

    fn identity() -> DeviceIdentity {
        DeviceIdentity::generate(DeviceId::generate())
    }

    fn record_for(identity: &DeviceIdentity, label: &str) -> PairedDevice {
        PairedDevice {
            device_id: identity.device_id(),
            static_public: identity.public_key(),
            label: label.to_string(),
            platform: Platform::Windows,
            addresses: vec![CandidateAddress::Lan("127.0.0.1:9000".parse().unwrap())],
            paired_at_ms: 0,
        }
    }

    fn handshake(
        local: &DeviceIdentity,
        local_trust: &Trust,
        remote: &DeviceIdentity,
        remote_trust: &Trust,
    ) -> Session {
        let (initiator, msg1) =
            HandshakeInitiator::start(local, local_trust, remote.device_id()).unwrap();
        let responder = HandshakeResponder::accept(remote, remote_trust, &msg1).unwrap();
        let (_remote_session, msg2) = responder.respond().unwrap();
        initiator.finish(&msg2).unwrap()
    }

    #[test]
    fn removing_a_peer_bumps_the_epoch_and_forgets_the_key() {
        let local = identity();
        let peer = identity();
        let mut trust = Trust::new(local.device_id());
        trust.add_peer(record_for(&peer, "peer"));
        assert_eq!(trust.epoch(), 0);

        trust.remove_peer(&peer.device_id()).unwrap();

        assert_eq!(trust.epoch(), 1);
        assert!(!trust.is_paired(&peer.device_id()));
        assert!(trust.authorize_static_key(&peer.public_key()).is_err());
    }

    #[test]
    fn removing_an_unknown_device_is_rejected() {
        let mut trust = Trust::new(DeviceId::generate());
        let stranger = DeviceId::generate();
        assert!(matches!(
            trust.remove_peer(&stranger),
            Err(Error::NotTrusted)
        ));
        assert_eq!(trust.epoch(), 0, "a failed removal must not bump the epoch");
    }

    /// The core rotation guarantee: a session set up before a removal is
    /// refused afterwards, the removed device's key stops authenticating at
    /// all, and — critically — the *other* remaining device can still talk
    /// to the local device by simply re-handshaking under the new epoch.
    #[test]
    fn removal_invalidates_old_sessions_but_remaining_devices_still_talk() {
        let local = identity();
        let removed = identity();
        let remaining = identity();

        let mut local_trust = Trust::new(local.device_id());
        local_trust.add_peer(record_for(&removed, "removed-device"));
        local_trust.add_peer(record_for(&remaining, "remaining-device"));

        let mut removed_trust = Trust::new(removed.device_id());
        removed_trust.add_peer(record_for(&local, "local"));

        let mut remaining_trust = Trust::new(remaining.device_id());
        remaining_trust.add_peer(record_for(&local, "local"));

        // A session with the soon-to-be-removed device, established while
        // it was still legitimately paired.
        let stale_session = handshake(&local, &local_trust, &removed, &removed_trust);
        assert!(
            local_trust.authorize_session(&stale_session).is_ok(),
            "valid before removal"
        );

        // The user removes it.
        local_trust.remove_peer(&removed.device_id()).unwrap();

        // The old session is now refused, even though the Noise keys inside
        // it are technically still intact.
        assert!(matches!(
            local_trust.authorize_session(&stale_session),
            Err(Error::NotTrusted)
        ));

        // The removed device's static key cannot start a fresh handshake
        // either — `remaining_trust` was never involved, but from
        // `local_trust`'s point of view the key is simply gone.
        assert!(matches!(
            local_trust.authorize_static_key(&removed.public_key()),
            Err(Error::NotTrusted)
        ));

        // The device that was *not* removed re-handshakes under the new
        // epoch without any special-casing and is authorized normally.
        let fresh_session = handshake(&local, &local_trust, &remaining, &remaining_trust);
        assert_eq!(fresh_session.epoch(), local_trust.epoch());
        assert!(local_trust.authorize_session(&fresh_session).is_ok());

        let ciphertext_marker = b"still works";
        // Sanity: the still-paired session is a real, usable channel, not
        // just an epoch number that happens to match.
        let mut fresh_session = fresh_session;
        let ct = fresh_session.write_message(ciphertext_marker).unwrap();
        assert_ne!(ct, ciphertext_marker);
    }
}
