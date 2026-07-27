//! Offer-and-chunk transfer for payloads too big to inline.
//!
//! A large image travels on its own stream so it cannot stall control traffic,
//! and the receiver asks for a starting index rather than the whole thing —
//! which is what makes a laptop that slept mid-transfer continue instead of
//! starting the 40 MB again.
//!
//! Nothing here does I/O. The sender asks [`ChunkSource`] which byte range a
//! chunk covers; the receiver feeds bytes to [`ChunkReceiver`] and gets the
//! assembled blob back only if it hashes to what was promised.

use std::ops::Range;

use clipse_core::ContentHash;

/// 256 KiB. Large enough that a 40 MB image is ~160 messages rather than
/// thousands, small enough that a lost connection costs little work.
pub const DEFAULT_CHUNK_BYTES: usize = 256 * 1024;

/// Ceiling on a single advertised payload. A peer claiming a 100 GB blob must
/// be refused before anything allocates on its say-so.
pub const MAX_BLOB_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ChunkError {
    #[error("offered blob of {size} bytes exceeds the {MAX_BLOB_BYTES} byte limit")]
    TooLarge { size: u64 },

    #[error("chunk size must be greater than zero")]
    ZeroChunkSize,

    #[error("chunk {index} is out of range for a {total}-chunk transfer")]
    OutOfRange { index: u32, total: u32 },

    #[error("chunk {index} carried {got} bytes, expected {expected}")]
    WrongLength {
        index: u32,
        got: usize,
        expected: usize,
    },

    #[error("transfer is missing {missing} chunk(s)")]
    Incomplete { missing: u32 },

    /// Deliberately says nothing about which chunk was wrong: the bytes are
    /// discarded wholesale, and a per-chunk diagnosis would only help someone
    /// probing for a way to slip content past the check.
    #[error("assembled bytes do not match the promised digest")]
    DigestMismatch,
}

/// Sender side: the shape of a transfer. Owns no bytes — the daemon streams
/// them from the blob store one range at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkSource {
    size: u64,
    chunk_bytes: usize,
}

impl ChunkSource {
    pub fn new(size: u64, chunk_bytes: usize) -> Result<Self, ChunkError> {
        if chunk_bytes == 0 {
            return Err(ChunkError::ZeroChunkSize);
        }
        if size > MAX_BLOB_BYTES {
            return Err(ChunkError::TooLarge { size });
        }
        Ok(Self { size, chunk_bytes })
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn chunk_bytes(&self) -> usize {
        self.chunk_bytes
    }

    pub fn total_chunks(&self) -> u32 {
        if self.size == 0 {
            return 0;
        }
        self.size.div_ceil(self.chunk_bytes as u64) as u32
    }

    /// Byte range covered by `index`, or `None` past the end.
    pub fn range(&self, index: u32) -> Option<Range<usize>> {
        if index >= self.total_chunks() {
            return None;
        }
        let start = index as usize * self.chunk_bytes;
        let end = (start + self.chunk_bytes).min(self.size as usize);
        Some(start..end)
    }
}

/// Receiver side: collects chunks in any order and verifies the whole thing.
pub struct ChunkReceiver {
    digest: ContentHash,
    source: ChunkSource,
    buffer: Vec<u8>,
    received: Vec<bool>,
}

impl ChunkReceiver {
    pub fn new(digest: ContentHash, size: u64, chunk_bytes: usize) -> Result<Self, ChunkError> {
        let source = ChunkSource::new(size, chunk_bytes)?;
        Ok(Self {
            digest,
            source,
            // Allocating up front is safe: `ChunkSource::new` has already
            // refused anything above MAX_BLOB_BYTES.
            buffer: vec![0u8; size as usize],
            received: vec![false; source.total_chunks() as usize],
        })
    }

    pub fn digest(&self) -> &ContentHash {
        &self.digest
    }

    pub fn total_chunks(&self) -> u32 {
        self.source.total_chunks()
    }

    pub fn received_chunks(&self) -> u32 {
        self.received.iter().filter(|got| **got).count() as u32
    }

    pub fn is_complete(&self) -> bool {
        self.received.iter().all(|got| *got)
    }

    /// The index to ask a peer to resume from: the first chunk still missing.
    pub fn resume_from(&self) -> u32 {
        self.received
            .iter()
            .position(|got| !*got)
            .unwrap_or(self.received.len()) as u32
    }

    /// Take one chunk. Re-delivering a chunk we already have is fine — that is
    /// what happens whenever a transfer resumes slightly behind where it
    /// stopped.
    pub fn accept(&mut self, index: u32, bytes: &[u8]) -> Result<(), ChunkError> {
        let Some(range) = self.source.range(index) else {
            return Err(ChunkError::OutOfRange {
                index,
                total: self.source.total_chunks(),
            });
        };

        let expected = range.len();
        if bytes.len() != expected {
            return Err(ChunkError::WrongLength {
                index,
                got: bytes.len(),
                expected,
            });
        }

        self.buffer[range].copy_from_slice(bytes);
        self.received[index as usize] = true;
        Ok(())
    }

    /// Assemble and verify. Consumes the receiver either way: a transfer that
    /// failed its digest must not be retried chunk-by-chunk against the same
    /// half-trusted buffer.
    pub fn finish(self) -> Result<Vec<u8>, ChunkError> {
        let missing = self.received.iter().filter(|got| !**got).count() as u32;
        if missing > 0 {
            return Err(ChunkError::Incomplete { missing });
        }

        if ContentHash::of(&self.buffer) != self.digest {
            return Err(ChunkError::DigestMismatch);
        }

        Ok(self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn receiver_for(bytes: &[u8], chunk: usize) -> ChunkReceiver {
        ChunkReceiver::new(ContentHash::of(bytes), bytes.len() as u64, chunk).unwrap()
    }

    #[test]
    fn chunk_count_covers_a_partial_last_chunk() {
        let source = ChunkSource::new(1000, 256).unwrap();
        assert_eq!(source.total_chunks(), 4);
        assert_eq!(source.range(3), Some(768..1000));
        assert_eq!(source.range(4), None);

        let exact = ChunkSource::new(1024, 256).unwrap();
        assert_eq!(exact.total_chunks(), 4);
        assert_eq!(exact.range(3), Some(768..1024));
    }

    #[test]
    fn an_empty_blob_has_no_chunks() {
        let source = ChunkSource::new(0, 256).unwrap();
        assert_eq!(source.total_chunks(), 0);
        assert_eq!(source.range(0), None);

        let receiver = ChunkReceiver::new(ContentHash::of(b""), 0, 256).unwrap();
        assert!(receiver.is_complete(), "nothing to wait for");
        assert_eq!(receiver.finish().unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn chunks_reassemble_in_order() {
        let bytes = blob(1000);
        let source = ChunkSource::new(bytes.len() as u64, 256).unwrap();
        let mut receiver = receiver_for(&bytes, 256);

        for index in 0..source.total_chunks() {
            let range = source.range(index).unwrap();
            receiver.accept(index, &bytes[range]).unwrap();
        }

        assert!(receiver.is_complete());
        assert_eq!(receiver.finish().unwrap(), bytes);
    }

    #[test]
    fn chunks_reassemble_out_of_order() {
        let bytes = blob(1000);
        let source = ChunkSource::new(bytes.len() as u64, 256).unwrap();
        let mut receiver = receiver_for(&bytes, 256);

        for index in [3u32, 0, 2, 1] {
            let range = source.range(index).unwrap();
            receiver.accept(index, &bytes[range]).unwrap();
        }

        assert_eq!(receiver.finish().unwrap(), bytes);
    }

    #[test]
    fn resume_reports_the_first_gap_not_the_count() {
        let bytes = blob(1000);
        let source = ChunkSource::new(bytes.len() as u64, 256).unwrap();
        let mut receiver = receiver_for(&bytes, 256);

        // A transfer that died after chunk 0 and then delivered 2 out of turn.
        for index in [0u32, 2] {
            let range = source.range(index).unwrap();
            receiver.accept(index, &bytes[range]).unwrap();
        }

        assert_eq!(receiver.resume_from(), 1, "must resume at the gap");
        assert_eq!(receiver.received_chunks(), 2);
        assert!(!receiver.is_complete());
    }

    #[test]
    fn a_resumed_transfer_may_repeat_chunks_it_already_has() {
        let bytes = blob(1000);
        let source = ChunkSource::new(bytes.len() as u64, 256).unwrap();
        let mut receiver = receiver_for(&bytes, 256);

        for index in 0..source.total_chunks() {
            let range = source.range(index).unwrap();
            receiver.accept(index, &bytes[range.clone()]).unwrap();
            // The peer resumed slightly behind and sent it again.
            receiver.accept(index, &bytes[range]).unwrap();
        }

        assert_eq!(receiver.finish().unwrap(), bytes);
    }

    #[test]
    fn an_incomplete_transfer_will_not_finish() {
        let bytes = blob(1000);
        let source = ChunkSource::new(bytes.len() as u64, 256).unwrap();
        let mut receiver = receiver_for(&bytes, 256);
        let range = source.range(0).unwrap();
        receiver.accept(0, &bytes[range]).unwrap();

        assert_eq!(
            receiver.finish(),
            Err(ChunkError::Incomplete { missing: 3 })
        );
    }

    #[test]
    fn corrupted_bytes_are_caught_and_nothing_is_returned() {
        let bytes = blob(1000);
        let source = ChunkSource::new(bytes.len() as u64, 256).unwrap();
        let mut receiver = receiver_for(&bytes, 256);

        for index in 0..source.total_chunks() {
            let range = source.range(index).unwrap();
            let mut chunk = bytes[range].to_vec();
            if index == 2 {
                chunk[0] ^= 0xFF; // one flipped bit on the wire
            }
            receiver.accept(index, &chunk).unwrap();
        }

        assert!(receiver.is_complete(), "every chunk arrived");
        assert_eq!(
            receiver.finish(),
            Err(ChunkError::DigestMismatch),
            "corrupted content must not be accepted"
        );
    }

    #[test]
    fn a_chunk_of_the_wrong_length_is_refused() {
        let bytes = blob(1000);
        let mut receiver = receiver_for(&bytes, 256);

        assert_eq!(
            receiver.accept(0, &bytes[0..100]),
            Err(ChunkError::WrongLength {
                index: 0,
                got: 100,
                expected: 256
            })
        );
        assert_eq!(
            receiver.accept(3, &bytes[0..256]),
            Err(ChunkError::WrongLength {
                index: 3,
                got: 256,
                expected: 232
            }),
            "the last chunk is short and must be checked against its own length"
        );
    }

    #[test]
    fn a_chunk_past_the_end_is_refused() {
        let bytes = blob(1000);
        let mut receiver = receiver_for(&bytes, 256);
        assert_eq!(
            receiver.accept(9, &bytes[0..256]),
            Err(ChunkError::OutOfRange { index: 9, total: 4 })
        );
    }

    #[test]
    fn an_absurd_offer_is_refused_before_allocating() {
        assert_eq!(
            ChunkSource::new(MAX_BLOB_BYTES + 1, 256),
            Err(ChunkError::TooLarge {
                size: MAX_BLOB_BYTES + 1
            })
        );
        assert!(
            ChunkReceiver::new(ContentHash::of(b""), MAX_BLOB_BYTES + 1, 256).is_err(),
            "a peer must not be able to make us allocate half a terabyte"
        );
    }

    #[test]
    fn a_zero_chunk_size_is_refused_rather_than_looping_forever() {
        assert_eq!(ChunkSource::new(100, 0), Err(ChunkError::ZeroChunkSize));
    }

    #[test]
    fn a_realistic_image_round_trips_at_the_default_chunk_size() {
        let bytes = blob(3 * DEFAULT_CHUNK_BYTES + 7_919);
        let source = ChunkSource::new(bytes.len() as u64, DEFAULT_CHUNK_BYTES).unwrap();
        assert_eq!(source.total_chunks(), 4);

        let mut receiver = receiver_for(&bytes, DEFAULT_CHUNK_BYTES);
        for index in 0..source.total_chunks() {
            let range = source.range(index).unwrap();
            receiver.accept(index, &bytes[range]).unwrap();
        }
        assert_eq!(receiver.finish().unwrap(), bytes);
    }
}
