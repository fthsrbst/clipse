use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// All failure modes for this crate collapse into a small, coarse set.
///
/// Deliberately coarse: a peer that sends a bad MAC and a peer that signs
/// with the wrong key both surface as [`Error::HandshakeFailed`] or
/// [`Error::DecryptFailed`]. Distinguishing "your key is wrong" from "your
/// ciphertext is corrupt" in the wire-visible error would hand an attacker a
/// free oracle for probing which part of a forged message it got right —
/// classic padding-oracle-shaped mistake, just one level up the stack. Keep
/// the interesting detail (if any) in `tracing` logs on the side that can
/// afford to know it, never in the `Display` string that might cross a
/// trust boundary.
#[derive(Debug, Error)]
pub enum Error {
    /// A Noise handshake could not be built, advanced or completed. Covers
    /// malformed messages, DH failures and authentication failures alike.
    #[error("handshake failed")]
    HandshakeFailed,

    /// A transport message failed to decrypt. Covers a flipped byte, a
    /// replayed/reordered message and a wrong key alike.
    #[error("decrypt failed")]
    DecryptFailed,

    /// The peer's static key is not in the local trust set, or the session
    /// was bound to an epoch the trust set has since moved past.
    #[error("peer is not trusted")]
    NotTrusted,

    /// A pairing offer, accept or confirm message failed a structural,
    /// freshness or commitment check.
    #[error("pairing failed")]
    PairingFailed,

    /// A pairing offer's `expires_at` has passed relative to the caller's
    /// clock.
    #[error("pairing offer expired")]
    PairingExpired,

    /// A QR/URI payload did not parse: wrong scheme, truncated, bad
    /// base64, or wrong protocol version.
    #[error("malformed pairing payload")]
    MalformedPayload,

    /// Session usage exceeded its rekey thresholds and must be rekeyed
    /// before continuing.
    #[error("session needs rekeying")]
    RekeyRequired,
}
