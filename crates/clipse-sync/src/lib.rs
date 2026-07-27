//! The rules two Clipse daemons follow when they reconcile histories.
//!
//! Everything here is pure: no sockets, no database, no async. `clipse-net`
//! carries the messages and `clipsed` applies the decisions, which is what
//! makes the interesting parts — what wins a conflict, when a clip must not be
//! re-broadcast, how a 40 MB image is reassembled — testable without two
//! machines and a network.
//!
//! The wire format is specified in `docs/sync-protocol.md`. Change that first.

pub mod chunk;
pub mod loop_guard;
pub mod merge;
pub mod message;

pub use chunk::{ChunkError, ChunkReceiver, ChunkSource, DEFAULT_CHUNK_BYTES};
pub use loop_guard::{LoopGuard, RebroadcastVerdict};
pub use merge::{MergeAction, merge};
pub use message::{ClipSummary, SyncMessage};
