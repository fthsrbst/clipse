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

/// The port a Clipse daemon binds when it can.
///
/// The endpoint used to bind port 0 and advertise whatever the OS handed out.
/// That works right up until a device restarts: every peer holds the *old*
/// ephemeral port, mDNS has not re-announced yet (or cannot, on a tailnet),
/// and the two devices are unreachable to each other for as long as it takes
/// discovery to catch up. A fixed port makes the address recorded at pairing
/// time keep working across restarts, which is also what lets pairing find a
/// device over a tailnet, where there is no multicast to ask.
///
/// Falling back to an ephemeral port when this one is taken is deliberate: two
/// daemons on one machine (a dev instance beside the installed app) must both
/// still run.
pub const DEFAULT_SYNC_PORT: u16 = 7420;

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
pub use quic::{Inbound, PairingCall, PairingExchange, PeerLink, QuicError, QuicTransport};
pub use tailnet::{TailnetError, TailnetPeer, TailnetStatus};
pub use transport::{AttemptFailure, Backoff, DialError, LinkError, LinkInfo};
