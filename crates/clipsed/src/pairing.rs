//! The pairing ceremony, as the daemon sees it.
//!
//! Three states, and the transitions between them are the security boundary:
//!
//! 1. **Idle** — inbound pairing attempts are refused.
//! 2. **Offering** — the user is looking at six digits. Lookups that do not
//!    match the code are refused and *counted*; a handful of wrong guesses
//!    cancels the offer, which is what keeps a six-digit secret meaningful
//!    against someone on the same network.
//! 3. **Proving** — the far side answered, we have sent our proof, and we are
//!    waiting for theirs. **Nothing is trusted yet.** The pairing is committed
//!    only when the responder's MAC verifies — that is the moment a device
//!    enters the trust set, and it is a machine check, not a human one.
//!
//! There is no "the user compares two codes" step any more. `clipse-crypto`
//! guarantees that a man-in-the-middle who substitutes a static key cannot
//! produce a MAC either side accepts, so the check that used to depend on
//! someone actually looking now happens here. See that crate's module docs for
//! what this does and does not defend against.

use std::time::{SystemTime, UNIX_EPOCH};

use clipse_crypto::{
    CandidateAddress, DeviceIdentity, MAX_LOOKUP_ATTEMPTS, PairedDevice, PairingAccept,
    PairingAwaitingProof, PairingCode, PairingConfirm, PairingFinish, PairingInitiator,
    PairingOffer, Platform,
};
use tracing::debug;

/// Collapse any failure in the ceremony to `Failed`, but log which step it was
/// on the way out.
///
/// The pairing surface deliberately says only "pairing failed" — telling a
/// caller *why* a handshake did not verify is telling an attacker which half of
/// their guess was right. That leaves the operator with a message that
/// `clipse-crypto` renders identically, so nothing in the error itself says
/// whether a payload failed to parse, the network call failed, or a MAC did not
/// verify. This log line is the only place that distinction survives.
pub fn failed(step: &'static str, error: impl std::fmt::Display) -> PairingError {
    debug!(step, %error, "pairing failed");
    PairingError::Failed
}

#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("no pairing is in progress")]
    NotInProgress,

    #[error("a pairing is already in progress")]
    AlreadyInProgress,

    #[error("that pairing code has expired")]
    Expired,

    #[error("no device on this network is showing that code")]
    NotFound,

    #[error("pairing failed")]
    Failed,
}

/// What the daemon is doing about pairing right now.
#[derive(Default)]
pub enum PairingState {
    #[default]
    Idle,
    /// Showing six digits and willing to answer the device that knows them.
    Offering {
        initiator: Box<PairingInitiator>,
        expires_at_ms: u64,
        /// Wrong tags seen since this offer went up. See
        /// [`clipse_crypto::MAX_LOOKUP_ATTEMPTS`].
        wrong_lookups: u32,
    },
    /// We answered; the far side still has to prove it derived the same
    /// transcript. Nothing is trusted in this state.
    Proving { awaiting: Box<PairingAwaitingProof> },
    /// This device is the one doing the typing. Held so a second attempt is
    /// refused rather than interleaved.
    Answering,
}

/// What an inbound lookup should be answered with.
pub enum LookupOutcome {
    /// The tag matched: hand over the offer and stay in the ceremony.
    Offer(Box<PairingOffer>),
    /// Not us, or not now. Says nothing about which.
    Refuse,
}

impl PairingState {
    pub fn is_busy(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    /// Begin as the offering device. `addresses` are what a peer should dial
    /// us on. Returns the digits to put on screen and when they stop working.
    pub fn begin(
        &mut self,
        identity: &DeviceIdentity,
        label: String,
        addresses: Vec<CandidateAddress>,
    ) -> Result<(PairingCode, u64), PairingError> {
        if self.is_busy() {
            return Err(PairingError::AlreadyInProgress);
        }

        let now = now_ms();
        let initiator = PairingInitiator::create(identity, label, platform(), addresses, now);
        let code = *initiator.code();
        let expires_at_ms = initiator.offer().expires_at_ms;

        *self = Self::Offering {
            initiator: Box::new(initiator),
            expires_at_ms,
            wrong_lookups: 0,
        };
        Ok((code, expires_at_ms))
    }

    /// Claim the state for a ceremony this device is driving, so an inbound
    /// attempt and a second `PairWithCode` cannot run over it.
    pub fn begin_answering(&mut self) -> Result<(), PairingError> {
        if self.is_busy() {
            return Err(PairingError::AlreadyInProgress);
        }
        *self = Self::Answering;
        Ok(())
    }

    /// Somebody asked whether a tag is ours.
    ///
    /// A wrong tag is counted, and enough of them retire the offer: a lookup
    /// is a guess at the code, and an unbounded number of guesses would make
    /// six digits worth nothing on a network an attacker can reach.
    pub fn lookup(&mut self, tag: &[u8; 16]) -> LookupOutcome {
        self.expire_if_stale();
        let Self::Offering {
            initiator,
            wrong_lookups,
            ..
        } = self
        else {
            return LookupOutcome::Refuse;
        };

        if initiator.answers(tag) {
            return LookupOutcome::Offer(Box::new(initiator.offer().clone()));
        }

        *wrong_lookups += 1;
        if *wrong_lookups >= MAX_LOOKUP_ATTEMPTS {
            debug!("too many wrong pairing codes tried; the offer is cancelled");
            *self = Self::Idle;
        }
        LookupOutcome::Refuse
    }

    /// The far side sent its accept. Produces the confirm to send back and
    /// moves to waiting for its proof.
    pub fn answer_accept(
        &mut self,
        accept: &PairingAccept,
    ) -> Result<PairingConfirm, PairingError> {
        let Self::Offering {
            initiator,
            expires_at_ms,
            ..
        } = std::mem::take(self)
        else {
            return Err(PairingError::NotInProgress);
        };

        if now_ms() > expires_at_ms {
            return Err(PairingError::Expired);
        }

        let (confirm, awaiting) = initiator
            .accept(accept, now_ms())
            .map_err(|e| failed("initiator accept", e))?;

        *self = Self::Proving {
            awaiting: Box::new(awaiting),
        };
        Ok(confirm)
    }

    /// The far side's proof arrived. This is the only place a device the user
    /// did not type a code for can enter the trust set — and it cannot, because
    /// the MAC would not verify.
    pub fn finish(&mut self, finish: &PairingFinish) -> Result<PairedDevice, PairingError> {
        let Self::Proving { awaiting } = std::mem::take(self) else {
            return Err(PairingError::NotInProgress);
        };
        awaiting
            .verify(finish)
            .map_err(|e| failed("verify responder proof", e))
    }

    /// The user closed the screen, said no, or it timed out.
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

pub fn platform() -> Platform {
    match std::env::consts::OS {
        "windows" => Platform::Windows,
        "macos" => Platform::MacOs,
        "linux" => Platform::Linux,
        other => Platform::Other(other.to_string()),
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use clipse_core::DeviceId;
    use clipse_crypto::PairingResponder;

    use super::*;

    fn identity() -> DeviceIdentity {
        DeviceIdentity::generate(DeviceId::generate())
    }

    fn addr() -> Vec<CandidateAddress> {
        vec![CandidateAddress::Lan("127.0.0.1:7420".parse().unwrap())]
    }

    /// Drives the typing side against an offering `PairingState`, the way the
    /// two daemons do over one QUIC connection.
    fn run_ceremony(
        state: &mut PairingState,
        code: PairingCode,
        typist: &DeviceIdentity,
    ) -> Result<PairedDevice, PairingError> {
        let LookupOutcome::Offer(offer) = state.lookup(&code.tag()) else {
            return Err(PairingError::NotFound);
        };

        let (responder, accept) = PairingResponder::from_offer(
            *offer,
            code,
            typist,
            "typist".into(),
            platform(),
            addr(),
            now_ms(),
        )
        .map_err(|e| failed("build responder", e))?;

        let confirm = state.answer_accept(&accept)?;
        let (finish, _their_view) = responder
            .verify(&confirm, now_ms())
            .map_err(|e| failed("verify offer proof", e))?;
        state.finish(&finish)
    }

    /// A structurally valid `PairingFinish`, produced by a real ceremony
    /// between two other devices. Used to check that a state which is not in
    /// a ceremony refuses one outright rather than verifying it.
    fn a_valid_finish() -> PairingFinish {
        let offering = identity();
        let typist = identity();
        let mut state = PairingState::default();
        let (code, _) = state.begin(&offering, "elsewhere".into(), addr()).unwrap();

        let LookupOutcome::Offer(offer) = state.lookup(&code.tag()) else {
            panic!("the offering state must answer its own tag");
        };
        let (responder, accept) = PairingResponder::from_offer(
            *offer,
            code,
            &typist,
            "typist".into(),
            platform(),
            addr(),
            now_ms(),
        )
        .unwrap();
        let confirm = state.answer_accept(&accept).unwrap();
        responder.verify(&confirm, now_ms()).unwrap().0
    }

    #[test]
    fn an_idle_daemon_refuses_pairing_attempts() {
        let mut state = PairingState::default();
        assert!(matches!(state.lookup(&[0u8; 16]), LookupOutcome::Refuse));
        assert!(
            matches!(
                state.finish(&a_valid_finish()),
                Err(PairingError::NotInProgress)
            ),
            "a proof from someone else's ceremony must trust nobody"
        );
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

    #[test]
    fn a_typed_code_pairs_and_nothing_is_trusted_before_the_proof() {
        let offering = identity();
        let typist = identity();

        let mut state = PairingState::default();
        let (code, _) = state.begin(&offering, "offering".into(), addr()).unwrap();

        let paired = run_ceremony(&mut state, code, &typist).expect("honest ceremony");
        assert_eq!(paired.device_id, typist.device_id());
        assert_eq!(paired.static_public, typist.public_key());
        assert!(matches!(state, PairingState::Idle));
    }

    #[test]
    fn a_wrong_code_never_reaches_the_ceremony() {
        let offering = identity();
        let typist = identity();

        let mut state = PairingState::default();
        let (code, _) = state.begin(&offering, "offering".into(), addr()).unwrap();

        let mut wrong = code.digits();
        wrong[5] = (wrong[5] + 1) % 10;
        let wrong = PairingCode::parse(&wrong.map(|d| d.to_string()).join("")).unwrap();

        assert!(matches!(
            run_ceremony(&mut state, wrong, &typist),
            Err(PairingError::NotFound)
        ));
        assert!(
            matches!(state, PairingState::Offering { .. }),
            "one typo must not end the ceremony"
        );
    }

    /// The bound that keeps six digits meaningful: an attacker on the network
    /// gets a handful of guesses, not a million.
    #[test]
    fn repeated_wrong_codes_retire_the_offer() {
        let offering = identity();
        let mut state = PairingState::default();
        state.begin(&offering, "offering".into(), addr()).unwrap();

        for _ in 0..MAX_LOOKUP_ATTEMPTS {
            assert!(matches!(state.lookup(&[7u8; 16]), LookupOutcome::Refuse));
        }
        assert!(
            !matches!(state, PairingState::Offering { .. }),
            "an offer being guessed at must stop answering"
        );
    }

    #[test]
    fn a_cancelled_offer_trusts_nobody() {
        let offering = identity();
        let typist = identity();
        let mut state = PairingState::default();
        let (code, _) = state.begin(&offering, "offering".into(), addr()).unwrap();
        state.cancel();

        assert!(matches!(state, PairingState::Idle));
        assert!(matches!(
            run_ceremony(&mut state, code, &typist),
            Err(PairingError::NotFound)
        ));
    }

    #[test]
    fn an_abandoned_offer_stops_answering() {
        let identity = identity();
        let mut state = PairingState::default();
        state.begin(&identity, "a".into(), addr()).unwrap();

        // Force the offer into the past.
        if let PairingState::Offering { expires_at_ms, .. } = &mut state {
            *expires_at_ms = 0;
        }

        assert!(state.expire_if_stale(), "a stale offer must be dropped");
        assert!(!matches!(state, PairingState::Offering { .. }));
        assert!(matches!(state.lookup(&[0u8; 16]), LookupOutcome::Refuse));
    }
}
