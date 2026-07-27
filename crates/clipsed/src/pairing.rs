//! The pairing ceremony, as the daemon sees it.
//!
//! Three states, and the transitions between them are the security boundary:
//!
//! 1. **Idle** — inbound pairing attempts are refused.
//! 2. **Offering** — the user is looking at a QR code. Exactly one attempt is
//!    accepted, and only until the offer expires.
//! 3. **AwaitingConfirmation** — both devices have computed six digits and the
//!    user is comparing them. **Nothing is trusted yet.** The pairing is
//!    committed only when `ConfirmPairing { accept: true }` arrives, which is
//!    the daemon's proxy for "the human said the digits match".
//!
//! That last point is the whole design. `clipse-crypto` guarantees a
//! man-in-the-middle cannot make the two sides compute the *same* digits; it
//! cannot guarantee anyone looked. This module is what makes looking matter.

use std::time::{SystemTime, UNIX_EPOCH};

use clipse_crypto::{
    CandidateAddress, DeviceIdentity, PairedDevice, PairingAccept, PairingConfirm,
    PairingInitiator, PairingOffer, PairingResponder, Platform, Sas,
};

#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("no pairing is in progress")]
    NotInProgress,

    #[error("a pairing is already in progress")]
    AlreadyInProgress,

    #[error("that pairing code has expired")]
    Expired,

    #[error("pairing failed")]
    Failed,
}

/// What the daemon is doing about pairing right now.
#[derive(Default)]
pub enum PairingState {
    #[default]
    Idle,
    /// Showing a QR code and willing to accept one answer.
    Offering {
        initiator: Box<PairingInitiator>,
        expires_at_ms: u64,
    },
    /// Digits computed on both sides; waiting for the user.
    AwaitingConfirmation {
        peer: Box<PairedDevice>,
        digits: String,
    },
}

impl PairingState {
    pub fn is_offering(&self) -> bool {
        matches!(self, Self::Offering { .. })
    }

    pub fn is_awaiting_confirmation(&self) -> bool {
        matches!(self, Self::AwaitingConfirmation { .. })
    }

    /// Begin as the initiator. `addresses` are what a peer should dial us on.
    pub fn begin(
        &mut self,
        identity: &DeviceIdentity,
        label: String,
        addresses: Vec<CandidateAddress>,
    ) -> Result<(String, u64), PairingError> {
        if !matches!(self, Self::Idle) {
            return Err(PairingError::AlreadyInProgress);
        }

        let now = now_ms();
        let initiator = PairingInitiator::create(identity, label, platform(), addresses, now);
        let uri = initiator.to_uri();
        let expires_at_ms = initiator.offer().expires_at_ms;

        *self = Self::Offering {
            initiator: Box::new(initiator),
            expires_at_ms,
        };
        Ok((uri, expires_at_ms))
    }

    /// An answer arrived on the pairing ALPN. Produces the `PairingConfirm`
    /// bytes to send back, and moves to awaiting the user's comparison.
    pub fn accept_answer(&mut self, accept_bytes: &[u8]) -> Result<Vec<u8>, PairingError> {
        let Self::Offering {
            initiator,
            expires_at_ms,
        } = std::mem::take(self)
        else {
            return Err(PairingError::NotInProgress);
        };

        let now = now_ms();
        if now > expires_at_ms {
            return Err(PairingError::Expired);
        }

        let accept = PairingAccept::from_bytes(accept_bytes).map_err(|_| PairingError::Failed)?;
        let (confirm, sas, peer) = initiator
            .accept(&accept, now)
            .map_err(|_| PairingError::Failed)?;

        *self = Self::AwaitingConfirmation {
            peer: Box::new(peer),
            digits: format_sas(&sas),
        };
        Ok(confirm.to_bytes())
    }

    /// Answer someone else's QR code. `send` carries the accept bytes to them
    /// and returns their confirm bytes.
    pub async fn answer_offer<F, Fut>(
        &mut self,
        uri: &str,
        identity: &DeviceIdentity,
        label: String,
        addresses: Vec<CandidateAddress>,
        send: F,
    ) -> Result<(), PairingError>
    where
        F: FnOnce(Vec<CandidateAddress>, Vec<u8>) -> Fut,
        Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
    {
        if !matches!(self, Self::Idle) {
            return Err(PairingError::AlreadyInProgress);
        }

        let now = now_ms();
        // Where to reach them comes from the QR code itself, so it is read
        // before the responder consumes the URI.
        let peer_addresses = PairingOffer::from_uri(uri)
            .map_err(|_| PairingError::Failed)?
            .addresses;

        let (responder, accept) =
            PairingResponder::from_offer(uri, identity, label, platform(), addresses, now)
                .map_err(|_| PairingError::Failed)?;
        let confirm_bytes = send(peer_addresses, accept.to_bytes())
            .await
            .map_err(|_| PairingError::Failed)?;

        let confirm =
            PairingConfirm::from_bytes(&confirm_bytes).map_err(|_| PairingError::Failed)?;
        let (sas, peer) = responder
            .verify(&confirm, now_ms())
            .map_err(|_| PairingError::Failed)?;

        *self = Self::AwaitingConfirmation {
            peer: Box::new(peer),
            digits: format_sas(&sas),
        };
        Ok(())
    }

    /// The digits to show the user, if there are any.
    pub fn digits(&self) -> Option<(String, String)> {
        match self {
            Self::AwaitingConfirmation { peer, digits } => {
                Some((digits.clone(), peer.label.clone()))
            }
            _ => None,
        }
    }

    /// The user said the digits match. Returns the device to trust.
    pub fn confirm(&mut self) -> Result<PairedDevice, PairingError> {
        match std::mem::take(self) {
            Self::AwaitingConfirmation { peer, .. } => Ok(*peer),
            other => {
                *self = other;
                Err(PairingError::NotInProgress)
            }
        }
    }

    /// The user said no, or closed the screen, or it timed out.
    pub fn cancel(&mut self) {
        *self = Self::Idle;
    }

    /// Drop an offer nobody answered in time. Called before each use so an
    /// abandoned pairing screen cannot leave the daemon accepting forever.
    pub fn expire_if_stale(&mut self) -> bool {
        if let Self::Offering { expires_at_ms, .. } = self
            && now_ms() > *expires_at_ms
        {
            *self = Self::Idle;
            return true;
        }
        false
    }
}

fn format_sas(sas: &Sas) -> String {
    sas.to_string()
}

fn platform() -> Platform {
    match std::env::consts::OS {
        "windows" => Platform::Windows,
        "macos" => Platform::MacOs,
        "linux" => Platform::Linux,
        other => Platform::Other(other.to_string()),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use clipse_core::DeviceId;

    use super::*;

    fn identity() -> DeviceIdentity {
        DeviceIdentity::generate(DeviceId::generate())
    }

    fn addr() -> Vec<CandidateAddress> {
        vec![CandidateAddress::Lan("127.0.0.1:7420".parse().unwrap())]
    }

    #[test]
    fn an_idle_daemon_refuses_pairing_answers() {
        let mut state = PairingState::default();
        assert!(matches!(
            state.accept_answer(b"anything"),
            Err(PairingError::NotInProgress)
        ));
    }

    #[test]
    fn beginning_twice_is_refused() {
        let identity = identity();
        let mut state = PairingState::default();
        state.begin(&identity, "a".into(), addr()).unwrap();
        assert!(matches!(
            state.begin(&identity, "a".into(), addr()),
            Err(PairingError::AlreadyInProgress)
        ));
    }

    #[tokio::test]
    async fn a_full_ceremony_agrees_on_the_digits_and_only_then_commits() {
        let alice = identity();
        let bob = identity();

        let mut alice_state = PairingState::default();
        let (uri, _) = alice_state.begin(&alice, "alice".into(), addr()).unwrap();

        // Bob answers, with Alice's side wired in as the "network".
        let mut bob_state = PairingState::default();
        let alice_cell = std::sync::Arc::new(std::sync::Mutex::new(alice_state));
        let alice_for_send = std::sync::Arc::clone(&alice_cell);

        bob_state
            .answer_offer(
                &uri,
                &bob,
                "bob".into(),
                addr(),
                |_addrs, accept| async move {
                    alice_for_send
                        .lock()
                        .unwrap()
                        .accept_answer(&accept)
                        .map_err(|e| e.to_string())
                },
            )
            .await
            .unwrap();

        alice_state = std::sync::Arc::try_unwrap(alice_cell)
            .unwrap_or_else(|_| panic!("send closure outlived the ceremony"))
            .into_inner()
            .unwrap();

        let (alice_digits, bob_label) = alice_state.digits().expect("alice has digits");
        let (bob_digits, alice_label) = bob_state.digits().expect("bob has digits");
        assert_eq!(
            alice_digits, bob_digits,
            "the user would be comparing two different codes"
        );
        assert_eq!(bob_label, "bob");
        assert_eq!(alice_label, "alice");

        // Nothing is trusted until the user says so.
        assert!(alice_state.is_awaiting_confirmation());
        let paired_bob = alice_state.confirm().unwrap();
        assert_eq!(paired_bob.device_id, bob.device_id());
        assert!(matches!(alice_state, PairingState::Idle));
    }

    #[test]
    fn refusing_leaves_nothing_behind() {
        let identity = identity();
        let mut state = PairingState::default();
        state.begin(&identity, "a".into(), addr()).unwrap();
        state.cancel();

        assert!(matches!(state, PairingState::Idle));
        assert!(matches!(state.confirm(), Err(PairingError::NotInProgress)));
    }

    #[test]
    fn confirming_without_a_ceremony_cannot_trust_anything() {
        let mut state = PairingState::default();
        assert!(matches!(state.confirm(), Err(PairingError::NotInProgress)));
    }

    #[test]
    fn an_abandoned_offer_stops_accepting_answers() {
        let identity = identity();
        let mut state = PairingState::default();
        state.begin(&identity, "a".into(), addr()).unwrap();

        // Force the offer into the past.
        if let PairingState::Offering { expires_at_ms, .. } = &mut state {
            *expires_at_ms = 0;
        }

        assert!(state.expire_if_stale(), "a stale offer must be dropped");
        assert!(!state.is_offering());
        assert!(matches!(
            state.accept_answer(b"late"),
            Err(PairingError::NotInProgress)
        ));
    }
}
