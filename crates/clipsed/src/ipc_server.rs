//! Serves the IPC protocol to local user interfaces.
//!
//! One task per connection. A connection becomes an event subscriber when it
//! sends `Subscribe`; until then it only ever writes responses.
//!
//! The event receiver is created when the connection is *accepted*, not when
//! `Subscribe` arrives. A clip copied during the handshake is therefore still
//! delivered — otherwise a UI could open with a list that is already stale and
//! have no way to notice.

use std::sync::Arc;

use clipse_ipc::codec::{FrameError, read_frame, write_frame};
use clipse_ipc::protocol::{Event, Frame, FrameBody, Request, Response};
use clipse_ipc::transport::Listener;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, warn};

/// How many events a slow UI may fall behind before it is told it lagged.
/// Bounded on purpose: a stuck webview must not grow the daemon's memory.
const EVENT_BUFFER: usize = 256;

/// Everything the daemon can answer. Implemented by `Daemon`; the tests use a
/// fake so the connection plumbing can be exercised on its own.
#[async_trait::async_trait]
pub trait RequestHandler: Send + Sync + 'static {
    async fn handle(&self, request: Request) -> Response;
}

pub struct IpcServer<H: RequestHandler> {
    handler: Arc<H>,
    events: broadcast::Sender<Event>,
}

impl<H: RequestHandler> IpcServer<H> {
    pub fn new(handler: Arc<H>) -> Self {
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        Self { handler, events }
    }

    /// Handle for the rest of the daemon to publish events on.
    pub fn events(&self) -> broadcast::Sender<Event> {
        self.events.clone()
    }

    /// Accept until `shutdown` resolves. Connection failures are logged and the
    /// loop continues — one broken UI must not take the daemon down.
    pub async fn serve(
        self,
        mut listener: Listener,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        debug!("ipc server shutting down");
                        return;
                    }
                }
                accepted = listener.accept() => match accepted {
                    Ok(stream) => {
                        let handler = Arc::clone(&self.handler);
                        let events = self.events.subscribe();
                        tokio::spawn(async move {
                            if let Err(e) = serve_connection(stream, handler, events).await
                                && !matches!(e, FrameError::Closed)
                            {
                                warn!(error = %e, "ipc connection ended badly");
                            }
                        });
                    }
                    Err(e) => warn!(error = %e, "ipc accept failed"),
                },
            }
        }
    }
}

async fn serve_connection<S, H>(
    stream: S,
    handler: Arc<H>,
    mut events: broadcast::Receiver<Event>,
) -> Result<(), FrameError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
    H: RequestHandler,
{
    let (reader, mut writer) = tokio::io::split(stream);

    // `read_frame` is not cancel-safe: dropping it mid-frame would leave the
    // stream desynchronised. Giving it a task of its own keeps it off the
    // select! below.
    let (req_tx, mut req_rx) = mpsc::channel::<Frame>(16);
    let reader_task = tokio::spawn(async move {
        let mut reader = reader;
        loop {
            match read_frame(&mut reader).await {
                Ok(frame) => {
                    if req_tx.send(frame).await.is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });

    let mut subscribed = false;

    loop {
        tokio::select! {
            incoming = req_rx.recv() => {
                let Some(frame) = incoming else { break };
                let FrameBody::Request(request) = frame.body else {
                    debug!("client sent a non-request frame; ignoring");
                    continue;
                };

                if matches!(request, Request::Subscribe) {
                    subscribed = true;
                }

                let response = handler.handle(request).await;
                write_frame(&mut writer, &Frame::response(frame.id, response)).await?;
            }

            event = events.recv(), if subscribed => match event {
                Ok(event) => write_frame(&mut writer, &Frame::event(event)).await?,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(missed = n, "ui fell behind the event stream");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }

    reader_task.abort();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use clipse_core::{ClipId, DeviceId};
    use clipse_ipc::protocol::HistoryQuery;

    use super::*;

    #[derive(Default)]
    struct FakeHandler {
        seen: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl RequestHandler for FakeHandler {
        async fn handle(&self, request: Request) -> Response {
            self.seen.fetch_add(1, Ordering::SeqCst);
            match request {
                Request::Hello { .. } => Response::Hello {
                    daemon_version: "test".into(),
                    ipc_version: clipse_ipc::IPC_VERSION,
                    device: DeviceId::generate(),
                },
                Request::History(_) => Response::Clips(Vec::new()),
                _ => Response::Ok,
            }
        }
    }

    /// An in-memory duplex pair stands in for the platform transport, which
    /// `clipse-ipc` already covers with its own integration test.
    fn duplex() -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
        tokio::io::duplex(64 * 1024)
    }

    #[tokio::test]
    async fn answers_requests_in_order() {
        let (server_side, mut client) = duplex();
        let handler = Arc::new(FakeHandler::default());
        let (_tx, rx) = broadcast::channel(8);

        let task = tokio::spawn(serve_connection(server_side, Arc::clone(&handler), rx));

        for id in 1..=3u64 {
            write_frame(&mut client, &Frame::request(id, Request::History(HistoryQuery::page(5))))
                .await
                .unwrap();
            let response = read_frame(&mut client).await.unwrap();
            assert_eq!(response.id, id);
            assert!(matches!(response.body, FrameBody::Response(Response::Clips(_))));
        }

        assert_eq!(handler.seen.load(Ordering::SeqCst), 3);
        drop(client);
        let _ = task.await;
    }

    #[tokio::test]
    async fn a_connection_that_never_subscribes_is_never_pushed_to() {
        let (server_side, mut client) = duplex();
        let (tx, rx) = broadcast::channel(8);
        let task = tokio::spawn(serve_connection(server_side, Arc::new(FakeHandler::default()), rx));

        tx.send(Event::ClipRemoved(ClipId::generate())).unwrap();

        // The only frame this connection may ever see is its own response.
        write_frame(&mut client, &Frame::request(1, Request::Status)).await.unwrap();
        let frame = read_frame(&mut client).await.unwrap();
        assert!(matches!(frame.body, FrameBody::Response(_)));

        let nothing_more =
            tokio::time::timeout(std::time::Duration::from_millis(150), read_frame(&mut client))
                .await;
        assert!(nothing_more.is_err(), "an unsubscribed client was pushed an event");

        drop(client);
        let _ = task.await;
    }

    #[tokio::test]
    async fn subscribing_loses_no_event_since_the_connection_opened() {
        let (server_side, mut client) = duplex();
        let (tx, rx) = broadcast::channel(8);
        let task = tokio::spawn(serve_connection(server_side, Arc::new(FakeHandler::default()), rx));

        // A clip copied between accept and Subscribe: the UI is already
        // connected, so dropping this would leave its list silently stale.
        let during_handshake = ClipId::generate();
        tx.send(Event::ClipRemoved(during_handshake)).unwrap();

        write_frame(&mut client, &Frame::request(1, Request::Subscribe)).await.unwrap();
        let ack = read_frame(&mut client).await.unwrap();
        assert!(matches!(ack.body, FrameBody::Response(Response::Ok)));

        let after = ClipId::generate();
        tx.send(Event::ClipRemoved(after)).unwrap();

        let mut delivered = Vec::new();
        for _ in 0..2 {
            match read_frame(&mut client).await.unwrap().body {
                FrameBody::Event(Event::ClipRemoved(id)) => delivered.push(id),
                other => panic!("expected an event, got {other:?}"),
            }
        }
        assert_eq!(delivered, vec![during_handshake, after], "events arrived out of order");

        drop(client);
        let _ = task.await;
    }

    #[tokio::test]
    async fn a_client_that_disappears_does_not_hang_the_server() {
        let (server_side, client) = duplex();
        let (_tx, rx) = broadcast::channel(8);
        let task = tokio::spawn(serve_connection(server_side, Arc::new(FakeHandler::default()), rx));

        drop(client);

        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("connection task did not finish")
            .expect("task panicked")
            .expect("connection errored");
    }
}
