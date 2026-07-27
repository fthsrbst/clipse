//! The seam a transport plugs into.
//!
//! `clipse-sync` decides *what* to say; a transport decides how the bytes
//! get there. Keeping the two apart is what lets a future transport
//! (Bluetooth, someone's own relay) be added without the sync engine
//! changing, and it is why the QUIC implementation lives behind this rather
//! than being called directly.

use std::net::SocketAddr;

use clipse_core::DeviceId;

use crate::candidate::Reachability;

/// Why a dial did not produce a link.
///
/// The distinction between these two is the whole point of the type. An
/// unreachable peer is ordinary — a laptop is asleep, a network changed — and
/// should be retried patiently. A *rejected* peer means the other side would
/// not accept our identity, which is either a device the user removed or
/// something that deserves a human's attention; retrying that in a loop would
/// hide it.
#[derive(Debug, thiserror::Error)]
pub enum DialError {
    #[error("no candidate address answered ({} tried)", attempts.len())]
    Unreachable { attempts: Vec<AttemptFailure> },

    #[error("the peer refused our identity: {source}")]
    Rejected {
        #[source]
        source: clipse_crypto::Error,
    },

    #[error("that device is not paired")]
    NotPaired,

    #[error("peer has no known addresses")]
    NoCandidates,
}

impl DialError {
    /// Should this peer be dialled again later?
    ///
    /// Called by the reconnect loop. `Rejected` and `NotPaired` are terminal
    /// until the user does something about them.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Unreachable { .. })
    }
}

#[derive(Debug, Clone)]
pub struct AttemptFailure {
    pub addr: SocketAddr,
    pub reachability: Reachability,
    pub reason: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("the peer closed the connection")]
    Closed,

    #[error("transport: {0}")]
    Transport(String),

    #[error("crypto: {0}")]
    Crypto(#[from] clipse_crypto::Error),

    #[error("could not encode a message: {0}")]
    Encode(#[from] rmp_serde::encode::Error),

    #[error("malformed message from peer: {0}")]
    Decode(#[from] rmp_serde::decode::Error),

    #[error("message of {size} bytes exceeds the {max} byte limit")]
    TooLarge { size: u64, max: u64 },
}

/// Identity and route of a connected peer, for the UI and for logging.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkInfo {
    pub device: DeviceId,
    pub addr: SocketAddr,
    pub reachability: Reachability,
}

/// Capped exponential backoff for reconnecting to an unreachable peer.
///
/// Capped rather than unbounded because a laptop that has been shut for a week
/// should still be found within a minute of waking, not after the delay has
/// grown to an hour.
#[derive(Clone, Debug)]
pub struct Backoff {
    base_ms: u64,
    max_ms: u64,
    attempt: u32,
}

impl Backoff {
    pub fn new(base_ms: u64, max_ms: u64) -> Self {
        Self {
            base_ms,
            max_ms,
            attempt: 0,
        }
    }

    /// How long to wait before the next attempt, and advance.
    pub fn next_delay_ms(&mut self) -> u64 {
        // Saturating shift: after ~6 attempts this pins at max_ms, and the
        // shift itself must not overflow on a peer that has been down for
        // days.
        let factor = 1u64.checked_shl(self.attempt).unwrap_or(u64::MAX);
        let delay = self.base_ms.saturating_mul(factor).min(self.max_ms);
        self.attempt = self.attempt.saturating_add(1);
        delay
    }

    /// Called after a successful connection.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    pub fn attempts(&self) -> u32 {
        self.attempt
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(500, 60_000)
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    #[test]
    fn an_unreachable_peer_is_retried_and_a_rejected_one_is_not() {
        let unreachable = DialError::Unreachable {
            attempts: vec![AttemptFailure {
                addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1),
                reachability: Reachability::Lan,
                reason: "timed out".into(),
            }],
        };
        assert!(unreachable.is_retryable());

        let rejected = DialError::Rejected {
            source: clipse_crypto::Error::NotTrusted,
        };
        assert!(
            !rejected.is_retryable(),
            "a removed device must surface, not spin in a retry loop"
        );
        assert!(!DialError::NotPaired.is_retryable());
        assert!(!DialError::NoCandidates.is_retryable());
    }

    #[test]
    fn backoff_grows_then_caps() {
        let mut backoff = Backoff::new(500, 8_000);
        assert_eq!(backoff.next_delay_ms(), 500);
        assert_eq!(backoff.next_delay_ms(), 1_000);
        assert_eq!(backoff.next_delay_ms(), 2_000);
        assert_eq!(backoff.next_delay_ms(), 4_000);
        assert_eq!(backoff.next_delay_ms(), 8_000);
        assert_eq!(backoff.next_delay_ms(), 8_000, "must not grow past the cap");
    }

    #[test]
    fn backoff_resets_after_a_success() {
        let mut backoff = Backoff::new(500, 8_000);
        backoff.next_delay_ms();
        backoff.next_delay_ms();
        backoff.reset();
        assert_eq!(backoff.next_delay_ms(), 500);
        assert_eq!(backoff.attempts(), 1);
    }

    #[test]
    fn a_peer_down_for_days_does_not_overflow_the_shift() {
        let mut backoff = Backoff::new(500, 60_000);
        for _ in 0..10_000 {
            let delay = backoff.next_delay_ms();
            assert!(delay <= 60_000, "delay escaped the cap: {delay}");
        }
    }

    #[test]
    fn the_failure_list_says_which_addresses_were_tried() {
        let error = DialError::Unreachable {
            attempts: vec![
                AttemptFailure {
                    addr: "192.168.1.10:7420".parse().unwrap(),
                    reachability: Reachability::Lan,
                    reason: "connection refused".into(),
                },
                AttemptFailure {
                    addr: "100.64.0.7:7420".parse().unwrap(),
                    reachability: Reachability::Tailnet,
                    reason: "timed out".into(),
                },
            ],
        };
        assert!(error.to_string().contains('2'), "{error}");
    }
}
