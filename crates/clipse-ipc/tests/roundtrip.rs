//! Proves the real platform transport works: a listener, a client, a
//! handshake, a command and a pushed event — over a named pipe on Windows and
//! a unix socket everywhere else.

use clipse_core::{DeviceId, Paths};
use clipse_ipc::codec::{read_frame, write_frame};
use clipse_ipc::protocol::{
    CaptureMode, DaemonStatus, Event, Frame, FrameBody, HistoryQuery, Request, Response,
};
use clipse_ipc::transport::Listener;
use clipse_ipc::{Client, IPC_VERSION};

fn status(device: DeviceId) -> DaemonStatus {
    DaemonStatus {
        device,
        device_label: "test".into(),
        daemon_version: "0.1.0".into(),
        paused: false,
        capture_mode: CaptureMode::Automatic,
        clip_count: 3,
        blob_bytes: 0,
        blob_quota_bytes: 1024,
        peers_online: 0,
        peers_total: 0,
        secrets_refused: 7,
    }
}

/// Minimal stand-in for `clipsed`: answers Hello and Status, then pushes one
/// event to a subscriber.
async fn serve_one(mut listener: Listener, device: DeviceId) {
    let mut stream = listener.accept().await.expect("accept");
    loop {
        let frame = match read_frame(&mut stream).await {
            Ok(f) => f,
            Err(_) => return,
        };
        let FrameBody::Request(request) = frame.body else {
            continue;
        };

        let response = match request {
            Request::Hello { .. } => Response::Hello {
                daemon_version: "0.1.0".into(),
                ipc_version: IPC_VERSION,
                device,
            },
            Request::Status => Response::Status(Box::new(status(device))),
            Request::History(_) => Response::Clips(Vec::new()),
            Request::Subscribe => Response::Ok,
            _ => Response::Ok,
        };
        let subscribed = matches!(request, Request::Subscribe);

        write_frame(&mut stream, &Frame::response(frame.id, response))
            .await
            .unwrap();

        if subscribed {
            write_frame(
                &mut stream,
                &Frame::event(Event::Suppressed {
                    reason: "password manager".into(),
                }),
            )
            .await
            .unwrap();
        }
    }
}

#[tokio::test]
async fn handshake_command_and_event_over_the_real_transport() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_root(dir.path());
    paths.create_all().unwrap();
    let endpoint = paths.ipc_endpoint();
    let device = DeviceId::generate();

    let listener = Listener::bind(&endpoint).await.expect("bind");
    let server = tokio::spawn(serve_one(listener, device));

    let mut client = Client::connect(&endpoint, "test-ui")
        .await
        .expect("connect");

    match client.call(Request::Status).await.unwrap() {
        Response::Status(s) => {
            assert_eq!(s.device, device);
            assert_eq!(s.clip_count, 3);
        }
        other => panic!("unexpected: {other:?}"),
    }

    match client
        .call(Request::History(HistoryQuery::page(10)))
        .await
        .unwrap()
    {
        Response::Clips(clips) => assert!(clips.is_empty()),
        other => panic!("unexpected: {other:?}"),
    }

    let mut events = client.subscribe().await.expect("subscribe");
    match events.next().await.unwrap() {
        Event::Suppressed { reason } => assert_eq!(reason, "password manager"),
        other => panic!("unexpected: {other:?}"),
    }

    drop(events);
    let _ = server.await;
}

#[tokio::test]
async fn connecting_without_a_daemon_is_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_root(dir.path());
    paths.create_all().unwrap();

    let err = Client::connect(&paths.ipc_endpoint(), "test-ui")
        .await
        .unwrap_err();
    let text = err.to_string();
    assert!(text.contains("no Clipse daemon"), "unhelpful error: {text}");
}

#[tokio::test]
async fn a_second_daemon_refuses_to_bind() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::with_root(dir.path());
    paths.create_all().unwrap();
    let endpoint = paths.ipc_endpoint();

    let _first = Listener::bind(&endpoint).await.expect("first bind");
    let err = Listener::bind(&endpoint).await.unwrap_err();
    assert!(
        err.to_string().contains("already listening"),
        "second daemon was allowed in: {err}"
    );
}
