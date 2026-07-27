//! What two daemons say to each other.
//!
//! Encoded as MessagePack inside the Noise session. Specified in
//! `docs/sync-protocol.md` — change that document first, and bump
//! `clipse_core::PROTOCOL_VERSION` for anything that is not additive.

use clipse_core::{Clip, ClipKind, ContentHash, DeviceId, Hlc};
use serde::{Deserialize, Serialize};

/// Enough for a peer to decide whether it wants a clip, without shipping any
/// content to make that decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipSummary {
    pub hash: ContentHash,
    pub hlc: Hlc,
    pub kind: ClipKind,
    pub pinned: bool,
    pub deleted: bool,
    pub total_size: u64,
}

impl ClipSummary {
    pub fn of(clip: &Clip) -> Self {
        Self {
            hash: clip.hash,
            hlc: clip.hlc,
            kind: clip.kind,
            pinned: clip.pinned,
            deleted: clip.deleted,
            total_size: clip.total_size(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncMessage {
    /// First message on the control stream. `max_hlc` is how far the sender's
    /// history goes, which is what turns a reconnect into a resume.
    Hello {
        device: DeviceId,
        /// Trust epoch. A peer whose epoch is behind ours has been removed and
        /// re-added, or is replaying an old session; either way it must
        /// re-handshake.
        epoch: u64,
        protocol: u16,
        max_hlc: Option<Hlc>,
        label: String,
        platform: String,
    },

    /// A page of what the sender has. `complete` is false until the last page.
    Summary {
        entries: Vec<ClipSummary>,
        complete: bool,
    },

    /// The hashes the receiver does not have, or has with an older HLC.
    Want {
        hashes: Vec<ContentHash>,
    },

    /// A whole clip. Payloads at or below `INLINE_MAX_BYTES` ride along;
    /// larger ones arrive as blob transfers.
    Push {
        clip: Box<Clip>,
    },

    BlobOffer {
        digest: ContentHash,
        size: u64,
        chunk_size: u32,
    },
    /// `from_chunk` is what makes a transfer resumable rather than restarted.
    BlobWant {
        digest: ContentHash,
        from_chunk: u32,
    },
    BlobChunk {
        digest: ContentHash,
        index: u32,
        #[serde(with = "serde_bytes_compat")]
        bytes: Vec<u8>,
    },
    BlobEnd {
        digest: ContentHash,
    },

    /// Highest HLC the sender has durably stored. Lets the peer advance its
    /// cursor so a reconnect does not re-offer everything.
    Ack {
        hlc: Hlc,
    },

    Bye {
        reason: String,
    },
}

/// `rmp-serde` encodes `Vec<u8>` as an array of integers by default, which
/// costs roughly two bytes per byte on a 256 KiB chunk. Round-tripping through
/// a byte-string keeps chunks the size they actually are.
mod serde_bytes_compat {
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Vec<u8>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a byte string")
            }

            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                Ok(v.to_vec())
            }

            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
                Ok(v)
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(byte) = seq.next_element::<u8>()? {
                    out.push(byte);
                }
                Ok(out)
            }
        }

        d.deserialize_bytes(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use clipse_core::{ClipFormat, ClipSource, Payload};

    use super::*;

    fn sample_clip() -> Clip {
        let device = DeviceId::generate();
        Clip::new(
            vec![
                Payload::new(ClipFormat::Text, b"hello".to_vec()),
                Payload::new(ClipFormat::Html, b"<b>hello</b>".to_vec()),
            ],
            ClipSource::new(device, "desktop"),
            Hlc::new(1_700_000_000_000, 3, device),
        )
    }

    fn roundtrip(message: &SyncMessage) -> SyncMessage {
        let bytes = rmp_serde::to_vec_named(message).unwrap();
        rmp_serde::from_slice(&bytes).unwrap()
    }

    #[test]
    fn summary_describes_a_clip_without_its_content() {
        let clip = sample_clip();
        let summary = ClipSummary::of(&clip);

        assert_eq!(summary.hash, clip.hash);
        assert_eq!(summary.hlc, clip.hlc);
        assert_eq!(summary.kind, clip.kind);
        assert_eq!(summary.total_size, clip.total_size());

        // The encoded summary must be far smaller than the clip itself — that
        // is the whole reason it exists.
        let summary_bytes = rmp_serde::to_vec_named(&summary).unwrap().len();
        let clip_bytes = rmp_serde::to_vec_named(&clip).unwrap().len();
        assert!(
            summary_bytes < clip_bytes,
            "summary {summary_bytes} was not smaller than clip {clip_bytes}"
        );
    }

    #[test]
    fn every_message_round_trips() {
        let device = DeviceId::generate();
        let hlc = Hlc::new(42, 1, device);
        let digest = ContentHash::of(b"blob");

        let messages = vec![
            SyncMessage::Hello {
                device,
                epoch: 7,
                protocol: clipse_core::PROTOCOL_VERSION,
                max_hlc: Some(hlc),
                label: "desktop".into(),
                platform: "windows".into(),
            },
            SyncMessage::Summary {
                entries: vec![ClipSummary::of(&sample_clip())],
                complete: false,
            },
            SyncMessage::Want {
                hashes: vec![digest],
            },
            SyncMessage::Push {
                clip: Box::new(sample_clip()),
            },
            SyncMessage::BlobOffer {
                digest,
                size: 1_000_000,
                chunk_size: 262_144,
            },
            SyncMessage::BlobWant {
                digest,
                from_chunk: 3,
            },
            SyncMessage::BlobChunk {
                digest,
                index: 3,
                bytes: vec![1, 2, 3, 4],
            },
            SyncMessage::BlobEnd { digest },
            SyncMessage::Ack { hlc },
            SyncMessage::Bye {
                reason: "going to sleep".into(),
            },
        ];

        for message in messages {
            assert_eq!(roundtrip(&message), message, "{message:?} did not survive");
        }
    }

    #[test]
    fn a_hello_with_no_history_round_trips() {
        let message = SyncMessage::Hello {
            device: DeviceId::generate(),
            epoch: 0,
            protocol: clipse_core::PROTOCOL_VERSION,
            max_hlc: None,
            label: "fresh install".into(),
            platform: "linux".into(),
        };
        assert_eq!(roundtrip(&message), message);
    }

    #[test]
    fn chunk_bytes_are_encoded_as_a_byte_string_not_an_integer_array() {
        let payload = vec![0xABu8; 64 * 1024];
        let message = SyncMessage::BlobChunk {
            digest: ContentHash::of(&payload),
            index: 0,
            bytes: payload.clone(),
        };

        let encoded = rmp_serde::to_vec_named(&message).unwrap();
        // An integer array would need at least two bytes per element for
        // values above 0x7f; a byte string is length-prefixed and pays one.
        assert!(
            encoded.len() < payload.len() * 2,
            "chunk encoding doubled the payload: {} bytes for {}",
            encoded.len(),
            payload.len()
        );
        assert_eq!(roundtrip(&message), message);
    }

    #[test]
    fn a_truncated_message_fails_to_decode_rather_than_half_parsing() {
        let message = SyncMessage::Ack {
            hlc: Hlc::new(1, 0, DeviceId::generate()),
        };
        let mut bytes = rmp_serde::to_vec_named(&message).unwrap();
        bytes.truncate(bytes.len() / 2);

        assert!(rmp_serde::from_slice::<SyncMessage>(&bytes).is_err());
    }
}
