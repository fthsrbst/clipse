//! Turning a [`SyncMessage`] into bytes on an encrypted stream, and back.
//!
//! A Noise transport message is capped at 65535 bytes, and a `Push` carrying
//! several inline payloads can be several hundred kilobytes, so one logical
//! message is not one Noise message. The plaintext is prefixed with its own
//! length and split into segments that each fit; the receiver decrypts
//! segments until it has the whole thing.
//!
//! This is separate from `quic` on purpose: it is the part with an off-by-one
//! risk, and it can be tested against two real `Session`s without a socket.

use clipse_crypto::Session;
use clipse_sync::SyncMessage;

use crate::transport::LinkError;

/// Plaintext bytes per Noise message. Noise allows 65535 total and the
/// ChaCha20-Poly1305 tag takes 16, so the ceiling is 65519; 60000 leaves room
/// without needing that arithmetic to be right.
const SEGMENT_BYTES: usize = 60_000;

/// Refuse a peer that claims an absurd message length before allocating for
/// it. Well above any legitimate `Summary` page or inline `Push`.
pub const MAX_MESSAGE_BYTES: u64 = 32 * 1024 * 1024;

/// Encrypt one message into the frames that go on the wire, in order.
///
/// Each frame is a self-contained Noise message; the caller writes each one
/// length-prefixed.
pub fn encode(session: &mut Session, message: &SyncMessage) -> Result<Vec<Vec<u8>>, LinkError> {
    let body = rmp_serde::to_vec_named(message)?;
    let total = body.len() as u64;
    if total > MAX_MESSAGE_BYTES {
        return Err(LinkError::TooLarge {
            size: total,
            max: MAX_MESSAGE_BYTES,
        });
    }

    let mut plaintext = Vec::with_capacity(body.len() + 4);
    plaintext.extend_from_slice(&(body.len() as u32).to_le_bytes());
    plaintext.extend_from_slice(&body);

    plaintext
        .chunks(SEGMENT_BYTES)
        .map(|segment| session.write_message(segment).map_err(LinkError::from))
        .collect()
}

/// Reassembles a message from decrypted segments.
#[derive(Debug, Default)]
pub struct Decoder {
    buffer: Vec<u8>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one frame straight off the wire. Returns the message once the last
    /// segment of one has arrived.
    pub fn accept(
        &mut self,
        session: &mut Session,
        frame: &[u8],
    ) -> Result<Option<SyncMessage>, LinkError> {
        let segment = session.read_message(frame)?;
        self.buffer.extend_from_slice(&segment);

        let Some(expected) = self.expected_len()? else {
            return Ok(None);
        };
        if self.buffer.len() < 4 + expected {
            return Ok(None);
        }

        let body = &self.buffer[4..4 + expected];
        let message: SyncMessage = rmp_serde::from_slice(body)?;
        // Anything past this message belongs to the next one: a segment can
        // in principle carry the tail of one and the head of another.
        self.buffer.drain(..4 + expected);
        Ok(Some(message))
    }

    fn expected_len(&self) -> Result<Option<usize>, LinkError> {
        if self.buffer.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as u64;
        if len > MAX_MESSAGE_BYTES {
            return Err(LinkError::TooLarge {
                size: len,
                max: MAX_MESSAGE_BYTES,
            });
        }
        Ok(Some(len as usize))
    }

    /// Bytes held for a partially received message, for diagnostics.
    pub fn pending(&self) -> usize {
        self.buffer.len()
    }
}

/// Two connected `Session`s, for tests in this crate that need real
/// encryption without a socket.
#[cfg(test)]
pub(crate) fn session_pair() -> (Session, Session) {
    use clipse_core::DeviceId;
    use clipse_crypto::{
        DeviceIdentity, HandshakeInitiator, HandshakeResponder, PairedDevice, Platform, Trust,
    };

    let a = DeviceIdentity::generate(DeviceId::generate());
    let b = DeviceIdentity::generate(DeviceId::generate());

    let paired = |id: &DeviceIdentity, label: &str| PairedDevice {
        device_id: id.device_id(),
        static_public: id.public_key(),
        label: label.to_string(),
        platform: Platform::Linux,
        addresses: vec![],
        paired_at_ms: 0,
    };

    let mut trust_a = Trust::new(a.device_id());
    trust_a.add_peer(paired(&b, "b"));
    let mut trust_b = Trust::new(b.device_id());
    trust_b.add_peer(paired(&a, "a"));

    let (initiator, first) = HandshakeInitiator::start(&a, &trust_a, b.device_id()).unwrap();
    let responder = HandshakeResponder::accept(&b, &trust_b, &first).unwrap();
    let (session_b, reply) = responder.respond().unwrap();
    let session_a = initiator.finish(&reply).unwrap();

    (session_a, session_b)
}

#[cfg(test)]
mod tests {
    use clipse_core::{Clip, ClipFormat, ClipSource, DeviceId, Hlc, Payload};

    use super::*;

    fn ping() -> SyncMessage {
        SyncMessage::Bye {
            reason: "goodnight".into(),
        }
    }

    /// A `Summary` page of `entries` clips.
    ///
    /// Deliberately not a `Push` with a huge inline payload: anything over
    /// `INLINE_MAX_BYTES` becomes a blob reference and the bytes never travel
    /// in the message at all, so a `Push` cannot actually get large. A summary
    /// page is what genuinely exceeds one Noise message in practice.
    fn big_summary(entries: usize) -> SyncMessage {
        let device = DeviceId::generate();
        SyncMessage::Summary {
            entries: (0..entries)
                .map(|i| {
                    let clip = Clip::new(
                        vec![Payload::new(
                            ClipFormat::Text,
                            format!("clip {i}").into_bytes(),
                        )],
                        ClipSource::new(device, "desktop"),
                        Hlc::new(i as u64, 0, device),
                    );
                    clipse_sync::ClipSummary::of(&clip)
                })
                .collect(),
            complete: true,
        }
    }

    fn encoded_len(message: &SyncMessage) -> usize {
        rmp_serde::to_vec_named(message).unwrap().len()
    }

    fn round_trip(message: &SyncMessage) -> SyncMessage {
        let (mut sender, mut receiver) = session_pair();
        let frames = encode(&mut sender, message).unwrap();

        let mut decoder = Decoder::new();
        let mut decoded = None;
        for frame in &frames {
            if let Some(message) = decoder.accept(&mut receiver, frame).unwrap() {
                assert!(decoded.is_none(), "one message must not decode twice");
                decoded = Some(message);
            }
        }
        assert_eq!(decoder.pending(), 0, "bytes left over after a full message");
        decoded.expect("no message decoded")
    }

    #[test]
    fn a_small_message_is_one_frame_and_round_trips() {
        let (mut sender, _) = session_pair();
        assert_eq!(encode(&mut sender, &ping()).unwrap().len(), 1);
        assert_eq!(round_trip(&ping()), ping());
    }

    #[test]
    fn a_message_larger_than_one_noise_message_is_segmented_and_reassembled() {
        // A Noise transport message caps at 65535 bytes; this page is well
        // past that, so a single-frame implementation would fail here.
        let message = big_summary(1_500);
        assert!(
            encoded_len(&message) > 3 * SEGMENT_BYTES,
            "test premise: the summary must exceed several segments, got {}",
            encoded_len(&message)
        );
        let (mut sender, _) = session_pair();
        let frames = encode(&mut sender, &message).unwrap();
        assert!(
            frames.len() > 3,
            "expected segmentation, got {}",
            frames.len()
        );
        assert!(
            frames.iter().all(|f| f.len() <= 65_535),
            "a frame exceeded the Noise message limit"
        );

        assert_eq!(round_trip(&message), message);
    }

    #[test]
    fn a_message_sitting_exactly_on_the_segment_boundary_round_trips() {
        // Walk entry counts until the encoding straddles an exact multiple of
        // SEGMENT_BYTES, then test on and around it — that is the off-by-one
        // segmentation code gets wrong.
        let mut checked = 0;
        for entries in 1..4_000 {
            let message = big_summary(entries);
            // +4 for the length prefix that shares the plaintext stream.
            let plaintext_len = encoded_len(&message) + 4;
            let distance = plaintext_len % SEGMENT_BYTES;
            if plaintext_len > SEGMENT_BYTES && (distance <= 40 || distance >= SEGMENT_BYTES - 40) {
                assert_eq!(round_trip(&message), message, "failed at {entries} entries");
                checked += 1;
                if checked == 3 {
                    return;
                }
            }
        }
        panic!("never landed near a segment boundary; the search is wrong");
    }

    #[test]
    fn several_messages_in_sequence_stay_distinct() {
        let (mut sender, mut receiver) = session_pair();
        let sent = vec![ping(), big_summary(1_200), ping()];

        let mut decoder = Decoder::new();
        let mut received = Vec::new();
        for message in &sent {
            for frame in encode(&mut sender, message).unwrap() {
                if let Some(decoded) = decoder.accept(&mut receiver, &frame).unwrap() {
                    received.push(decoded);
                }
            }
        }

        assert_eq!(received, sent);
        assert_eq!(decoder.pending(), 0);
    }

    #[test]
    fn a_tampered_frame_fails_to_decrypt_and_yields_nothing() {
        let (mut sender, mut receiver) = session_pair();
        let mut frames = encode(&mut sender, &ping()).unwrap();
        frames[0][10] ^= 0xFF;

        let mut decoder = Decoder::new();
        let result = decoder.accept(&mut receiver, &frames[0]);
        assert!(matches!(result, Err(LinkError::Crypto(_))), "{result:?}");
    }

    #[test]
    fn frames_replayed_out_of_order_are_refused() {
        // Noise's transport nonce is a bare counter, so ordering is the
        // transport's job. Reversing the frames must fail loudly rather than
        // producing plausible-looking bytes.
        let (mut sender, mut receiver) = session_pair();
        let mut frames = encode(&mut sender, &big_summary(1_500)).unwrap();
        frames.reverse();

        let mut decoder = Decoder::new();
        let mut failed = false;
        for frame in &frames {
            if decoder.accept(&mut receiver, frame).is_err() {
                failed = true;
                break;
            }
        }
        assert!(failed, "out-of-order frames were accepted");
    }

    #[test]
    fn an_absurd_declared_length_is_refused_before_allocating() {
        let (mut sender, mut receiver) = session_pair();
        // Hand-build a plaintext claiming a 4 GiB body.
        let mut plaintext = u32::MAX.to_le_bytes().to_vec();
        plaintext.extend_from_slice(b"nothing like that much");
        let frame = sender.write_message(&plaintext).unwrap();

        let mut decoder = Decoder::new();
        assert!(matches!(
            decoder.accept(&mut receiver, &frame),
            Err(LinkError::TooLarge { .. })
        ));
    }

    #[test]
    fn garbage_inside_a_valid_frame_is_a_decode_error() {
        let (mut sender, mut receiver) = session_pair();
        let body = b"not messagepack";
        let mut plaintext = (body.len() as u32).to_le_bytes().to_vec();
        plaintext.extend_from_slice(body);
        let frame = sender.write_message(&plaintext).unwrap();

        let mut decoder = Decoder::new();
        assert!(matches!(
            decoder.accept(&mut receiver, &frame),
            Err(LinkError::Decode(_))
        ));
    }
}
