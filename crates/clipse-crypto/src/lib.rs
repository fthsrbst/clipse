//! Device identity, pairing and encrypted sessions for Clipse.
//!
//! This crate is the entire trust foundation for a sync product with no
//! server and no account: it decides what "my other laptop" means
//! cryptographically, how two devices agree on that fact for the first time,
//! how they talk to each other afterwards, and what happens when the user
//! says a device is no longer theirs. Get any of the three wrong and the
//! product's central promise — "only my devices ever see my clipboard" — is
//! false regardless of what the UI claims.
//!
//! Deliberately pure: no I/O, no networking, no async. `clipse-net` drives
//! every state machine here by feeding it bytes it received over QUIC and
//! sending the bytes these modules produce in return. That separation is
//! what makes the MITM and replay tests in `pairing` and `session` possible
//! without a socket in sight — the whole protocol is a pile of pure
//! functions over byte slices.
#![forbid(unsafe_code)]

pub mod error;
pub mod identity;
pub mod pairing;
pub mod rotation;
pub mod session;

pub use error::{Error, Result};
pub use identity::{DeviceIdentity, DevicePublicKey, Fingerprint};
pub use pairing::{
    CandidateAddress, MAX_LOOKUP_ATTEMPTS, PAIRING_OFFER_TTL_SECS, PAIRING_PROTOCOL_VERSION,
    PairingAccept, PairingAwaitingProof, PairingCode, PairingConfirm, PairingFinish,
    PairingInitiator, PairingOffer, PairingResponder, PairingWire, Platform,
};
pub use rotation::{PairedDevice, Trust};
pub use session::{
    HandshakeInitiator, HandshakeResponder, REKEY_AFTER_BYTES, REKEY_AFTER_MESSAGES, Session,
};
