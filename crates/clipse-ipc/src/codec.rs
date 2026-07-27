//! Length-prefixed MessagePack framing.
//!
//! A 4-byte little-endian length followed by the encoded [`Frame`]. The length
//! prefix is checked before allocating: a client that claims a 4 GiB frame gets
//! an error, not an out-of-memory kill of the daemon.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::protocol::Frame;

/// Generous enough for a page of history with inline payloads, small enough
/// that a bad length prefix cannot exhaust memory. Large blobs never travel
/// through IPC — the UI asks for a blob path instead.
pub const MAX_FRAME_BYTES: u32 = 32 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("frame of {size} bytes exceeds the {MAX_FRAME_BYTES} byte limit")]
    TooLarge { size: u32 },

    #[error("malformed frame: {0}")]
    Decode(#[from] rmp_serde::decode::Error),

    #[error("could not encode frame: {0}")]
    Encode(#[from] rmp_serde::encode::Error),

    #[error("peer closed the connection")]
    Closed,
}

pub async fn write_frame<W>(writer: &mut W, frame: &Frame) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
{
    // `to_vec_named` keeps field names in the encoding. It costs a few bytes
    // per frame and buys us the ability to add optional fields without
    // breaking an older client mid-upgrade.
    let body = rmp_serde::to_vec_named(frame)?;
    let size = u32::try_from(body.len()).map_err(|_| FrameError::TooLarge { size: u32::MAX })?;
    if size > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge { size });
    }

    writer.write_all(&size.to_le_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_frame<R>(reader: &mut R) -> Result<Frame, FrameError>
where
    R: AsyncRead + Unpin,
{
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(e.into()),
    }

    let size = u32::from_le_bytes(len);
    if size > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge { size });
    }

    let mut body = vec![0u8; size as usize];
    reader.read_exact(&mut body).await?;
    Ok(rmp_serde::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use clipse_core::ClipId;

    use super::*;
    use crate::protocol::{Event, FrameBody, HistoryQuery, Request};

    #[tokio::test]
    async fn roundtrips_a_frame() {
        let mut buf = Vec::new();
        let sent = Frame::request(42, Request::History(HistoryQuery::page(10)));
        write_frame(&mut buf, &sent).await.unwrap();

        let mut cursor = std::io::Cursor::new(buf);
        let got = read_frame(&mut cursor).await.unwrap();
        assert_eq!(got.id, 42);
        assert!(matches!(got.body, FrameBody::Request(Request::History(q)) if q.limit == 10));
    }

    #[tokio::test]
    async fn reads_frames_back_to_back() {
        let mut buf = Vec::new();
        for i in 0..3u64 {
            write_frame(&mut buf, &Frame::request(i, Request::Status)).await.unwrap();
        }

        let mut cursor = std::io::Cursor::new(buf);
        for i in 0..3u64 {
            assert_eq!(read_frame(&mut cursor).await.unwrap().id, i);
        }
        assert!(matches!(read_frame(&mut cursor).await, Err(FrameError::Closed)));
    }

    #[tokio::test]
    async fn rejects_an_absurd_length_prefix_without_allocating() {
        let mut buf = u32::MAX.to_le_bytes().to_vec();
        buf.extend_from_slice(b"whatever");
        let mut cursor = std::io::Cursor::new(buf);
        assert!(matches!(
            read_frame(&mut cursor).await,
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[tokio::test]
    async fn rejects_garbage_body() {
        let body = b"not messagepack at all";
        let mut buf = (body.len() as u32).to_le_bytes().to_vec();
        buf.extend_from_slice(body);
        let mut cursor = std::io::Cursor::new(buf);
        assert!(matches!(read_frame(&mut cursor).await, Err(FrameError::Decode(_))));
    }

    #[tokio::test]
    async fn truncated_frame_is_an_error_not_a_hang() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &Frame::event(Event::ClipRemoved(ClipId::generate())))
            .await
            .unwrap();
        buf.truncate(buf.len() - 3);

        let mut cursor = std::io::Cursor::new(buf);
        assert!(read_frame(&mut cursor).await.is_err());
    }
}
