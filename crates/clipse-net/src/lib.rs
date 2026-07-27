//! How one Clipse daemon finds another.
//!
//! There is one transport (QUIC) and one ordered list of addresses to try, not
//! two separate networks — see `docs/decisions.md`. A tailnet address is just
//! another socket address; what differs is only where it came from and how far
//! down the dial order it sits.
//!
//! This crate is where a future transport (Bluetooth, someone's own relay)
//! would plug in: [`candidate::Reachability`] gains a variant and the dial
//! order gains a rule, and nothing in `clipse-sync` changes.

pub mod candidate;
pub mod discovery;
pub mod framing;
pub mod mdns;
pub mod quic;
pub mod tailnet;
pub mod transport;

pub use candidate::{Candidate, CandidateList, Reachability};
pub use discovery::{DiscoveredPeer, Discovery, DiscoveryError, DiscoveryEvent};
pub use mdns::{RecordError, SERVICE_TYPE, ServiceRecord};
pub use quic::{Inbound, PairingExchange, PeerLink, QuicError, QuicTransport};
pub use tailnet::{TailnetError, TailnetPeer, TailnetStatus};
pub use transport::{AttemptFailure, Backoff, DialError, LinkError, LinkInfo};
