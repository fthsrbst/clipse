//! A small client for talking to `clipsed`.
//!
//! Commands and events use separate connections. Multiplexing them over one
//! socket would need a correlation task and a pending-request map for no real
//! benefit: a UI issues a handful of commands per interaction, and the event
//! stream is naturally a long-lived read loop.

use crate::codec::{FrameError, read_frame, write_frame};
use crate::protocol::{ErrorCode, Event, Frame, FrameBody, IpcError, Request, Response};
use crate::transport::{IpcStream, TransportError, connect};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Transport(#[from] TransportError),

    #[error(transparent)]
    Frame(#[from] FrameError),

    #[error(transparent)]
    Daemon(#[from] IpcError),

    #[error("daemon speaks IPC version {daemon}, this client speaks {client}")]
    VersionMismatch { daemon: u16, client: u16 },

    #[error("daemon answered a {expected} request with something else")]
    UnexpectedResponse { expected: &'static str },
}

#[derive(Debug)]
pub struct Client {
    stream: IpcStream,
    next_id: u64,
}

impl Client {
    /// Connect and complete the handshake. Fails fast on a version mismatch so
    /// a stale UI cannot half-work against a newer daemon.
    pub async fn connect(endpoint: &str, client_name: &str) -> Result<Self, ClientError> {
        let stream = connect(endpoint).await?;
        let mut client = Self { stream, next_id: 1 };

        let hello = Request::Hello {
            client: client_name.to_string(),
            ipc_version: crate::IPC_VERSION,
        };
        match client.call(hello).await? {
            Response::Hello { ipc_version, .. } if ipc_version == crate::IPC_VERSION => Ok(client),
            Response::Hello { ipc_version, .. } => Err(ClientError::VersionMismatch {
                daemon: ipc_version,
                client: crate::IPC_VERSION,
            }),
            _ => Err(ClientError::UnexpectedResponse { expected: "Hello" }),
        }
    }

    /// Send a request and wait for its response.
    pub async fn call(&mut self, request: Request) -> Result<Response, ClientError> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);

        write_frame(&mut self.stream, &Frame::request(id, request)).await?;

        loop {
            let frame = read_frame(&mut self.stream).await?;
            match frame.body {
                FrameBody::Response(Response::Error(e)) => return Err(e.into()),
                FrameBody::Response(response) if frame.id == id => return Ok(response),
                // A subscribed command connection can still see an event
                // interleaved with a response; skip it rather than failing.
                FrameBody::Event(_) => continue,
                FrameBody::Response(_) => {
                    return Err(IpcError::new(
                        ErrorCode::Internal,
                        "response id did not match the request",
                    )
                    .into());
                }
                FrameBody::Request(_) => {
                    return Err(
                        IpcError::new(ErrorCode::BadRequest, "daemon sent a request").into(),
                    );
                }
            }
        }
    }

    /// Turn this connection into an event stream. Consumes the client because
    /// after `Subscribe` the daemon may push at any time.
    pub async fn subscribe(mut self) -> Result<EventStream, ClientError> {
        match self.call(Request::Subscribe).await? {
            Response::Ok => Ok(EventStream {
                stream: self.stream,
            }),
            _ => Err(ClientError::UnexpectedResponse {
                expected: "Subscribe",
            }),
        }
    }
}

#[derive(Debug)]
pub struct EventStream {
    stream: IpcStream,
}

impl EventStream {
    /// Next event, or an error when the daemon goes away.
    pub async fn next(&mut self) -> Result<Event, ClientError> {
        loop {
            let frame = read_frame(&mut self.stream).await?;
            match frame.body {
                FrameBody::Event(event) => return Ok(event),
                // Late response to a command issued before Subscribe.
                FrameBody::Response(_) | FrameBody::Request(_) => continue,
            }
        }
    }
}
