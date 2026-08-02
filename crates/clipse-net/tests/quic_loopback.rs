//! Two real QUIC endpoints on loopback: handshake, talk, refuse strangers.
//!
//! Nothing here is stubbed — these are the same `QuicTransport` and the same
//! Noise handshake the daemon uses. The only thing loopback changes is which
//! interface the packets cross.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use clipse_core::{Clip, ClipFormat, ClipSource, DeviceId, Hlc, Payload};
use clipse_crypto::{DeviceIdentity, PairedDevice, Platform, Trust};
use clipse_net::candidate::{Candidate, CandidateList, Reachability};
use clipse_net::{DialError, QuicTransport};
use clipse_sync::{ClipSummary, SyncMessage};

/// One device: its identity and the set of peers it trusts.
struct Device {
    identity: Arc<DeviceIdentity>,
    trust: Arc<RwLock<Trust>>,
}

impl Device {
    fn new() -> Self {
        let identity = Arc::new(DeviceIdentity::generate(DeviceId::generate()));
        let trust = Arc::new(RwLock::new(Trust::new(identity.device_id())));
        Self { identity, trust }
    }

    fn id(&self) -> DeviceId {
        self.identity.device_id()
    }

    fn trusts(&self, other: &Device, label: &str) {
        self.trust.write().unwrap().add_peer(PairedDevice {
            device_id: other.id(),
            static_public: other.identity.public_key(),
            label: label.to_string(),
            platform: Platform::Linux,
            addresses: vec![],
            paired_at_ms: 0,
        });
    }

    fn transport(&self) -> QuicTransport {
        QuicTransport::bind(
            "127.0.0.1:0".parse().unwrap(),
            Arc::clone(&self.identity),
            Arc::clone(&self.trust),
        )
        .expect("bind loopback endpoint")
    }
}

fn candidates(addr: SocketAddr) -> CandidateList {
    CandidateList::new([Candidate::lan(addr)])
}

fn hello(device: DeviceId) -> SyncMessage {
    SyncMessage::Hello {
        device,
        epoch: 0,
        protocol: clipse_core::PROTOCOL_VERSION,
        max_hlc: None,
        label: "test".into(),
        platform: "linux".into(),
    }
}

fn summary_page(entries: usize) -> SyncMessage {
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
                ClipSummary::of(&clip)
            })
            .collect(),
        complete: true,
    }
}

/// A pair of devices that trust each other, with the responder listening.
async fn paired_pair() -> (
    Device,
    Device,
    QuicTransport,
    Arc<QuicTransport>,
    SocketAddr,
) {
    let alice = Device::new();
    let bob = Device::new();
    alice.trusts(&bob, "bob");
    bob.trusts(&alice, "alice");

    let alice_transport = alice.transport();
    let bob_transport = Arc::new(bob.transport());
    let bob_addr = bob_transport.local_addr();

    (alice, bob, alice_transport, bob_transport, bob_addr)
}

#[tokio::test(flavor = "multi_thread")]
async fn two_paired_devices_handshake_and_exchange_messages() {
    let (alice, bob, alice_transport, bob_transport, bob_addr) = paired_pair().await;

    let server = tokio::spawn({
        let bob_transport = Arc::clone(&bob_transport);
        async move {
            let mut link = bob_transport
                .accept_session()
                .await
                .expect("endpoint closed")
                .expect("accept");

            // Bob answers whatever Alice says, twice, then a big page.
            let first = link.recv().await.unwrap();
            link.send(&first).await.unwrap();
            let second = link.recv().await.unwrap();
            link.send(&second).await.unwrap();
            link.send(&summary_page(1_500)).await.unwrap();
            link
        }
    });

    let mut link = alice_transport
        .dial(bob.id(), &candidates(bob_addr))
        .await
        .expect("dial should succeed between paired devices");

    assert_eq!(link.remote_device(), bob.id(), "wrong peer identity");
    assert_eq!(link.info().reachability, Reachability::Lan);

    let sent = hello(alice.id());
    link.send(&sent).await.unwrap();
    assert_eq!(link.recv().await.unwrap(), sent);

    let ack = SyncMessage::Ack {
        hlc: Hlc::new(7, 1, alice.id()),
    };
    link.send(&ack).await.unwrap();
    assert_eq!(link.recv().await.unwrap(), ack);

    // A message far larger than one Noise message must survive the trip.
    match link.recv().await.unwrap() {
        SyncMessage::Summary { entries, complete } => {
            assert_eq!(entries.len(), 1_500);
            assert!(complete);
        }
        other => panic!("expected a summary page, got {other:?}"),
    }

    link.close("done");
    let _ = server.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unpaired_device_is_refused_and_it_is_not_a_reachability_problem() {
    // Bob trusts Alice, but Alice was never told about Bob: IK needs the
    // responder's static key up front, so this fails before a packet is sent.
    let alice = Device::new();
    let bob = Device::new();
    bob.trusts(&alice, "alice");

    let alice_transport = alice.transport();
    let bob_transport = bob.transport();
    let bob_addr = bob_transport.local_addr();

    let error = alice_transport
        .dial(bob.id(), &candidates(bob_addr))
        .await
        .expect_err("an unpaired peer must not produce a link");

    assert!(matches!(error, DialError::NotPaired), "{error:?}");
    assert!(
        !error.is_retryable(),
        "an unpaired device must surface, not spin in a retry loop"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_that_reaches_us_is_rejected_at_the_noise_layer() {
    // The mirror image: the stranger knows our static key (it could have been
    // read off a QR code that was never confirmed) so it can send a valid IK
    // first message, but we have never paired with it.
    let bob = Device::new();
    let stranger = Device::new();
    stranger.trusts(&bob, "bob"); // one-directional: bob does not trust it

    let bob_transport = Arc::new(bob.transport());
    let bob_addr = bob_transport.local_addr();
    let stranger_transport = stranger.transport();

    let server = tokio::spawn({
        let bob_transport = Arc::clone(&bob_transport);
        async move {
            bob_transport
                .accept_session()
                .await
                .expect("endpoint closed")
        }
    });

    let dial = stranger_transport
        .dial(bob.id(), &candidates(bob_addr))
        .await;

    let accepted = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("accept did not settle")
        .expect("server task panicked");

    assert!(
        accepted.is_err(),
        "an untrusted peer completed a handshake with us"
    );
    // And the dialler learns it was refused rather than hanging.
    assert!(dial.is_err(), "the stranger believed it was connected");
}

#[tokio::test(flavor = "multi_thread")]
async fn dialling_falls_through_a_dead_address_to_a_live_one() {
    let (_alice, bob, alice_transport, bob_transport, bob_addr) = paired_pair().await;

    let server = tokio::spawn({
        let bob_transport = Arc::clone(&bob_transport);
        async move { bob_transport.accept_session().await }
    });

    // A LAN candidate nothing is listening on, and the real endpoint labelled
    // as the tailnet fallback. Both are loopback: what is under test is the
    // ordering and the fallback, not routing.
    let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let list = CandidateList::new([Candidate::lan(dead), Candidate::tailnet(bob_addr)]);

    let link = alice_transport
        .dial(bob.id(), &list)
        .await
        .expect("should have fallen through to the second candidate");

    assert_eq!(link.info().addr, bob_addr);
    assert_eq!(
        link.info().reachability,
        Reachability::Tailnet,
        "the winning candidate's reachability must be reported, not the first one tried"
    );

    link.close("done");
    let _ = server.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_with_no_addresses_is_not_a_connection_attempt() {
    let (_alice, bob, alice_transport, _bob_transport, _addr) = paired_pair().await;

    let error = alice_transport
        .dial(bob.id(), &CandidateList::default())
        .await
        .expect_err("dialling nowhere must fail");

    assert!(matches!(error, DialError::NoCandidates), "{error:?}");
    assert!(!error.is_retryable(), "there is nothing to retry against");
}

#[tokio::test(flavor = "multi_thread")]
async fn every_dead_address_is_reported_so_the_failure_can_be_diagnosed() {
    let (_alice, bob, alice_transport, _bob_transport, _addr) = paired_pair().await;

    let list = CandidateList::new([
        Candidate::lan("127.0.0.1:1".parse().unwrap()),
        Candidate::tailnet("127.0.0.1:2".parse().unwrap()),
    ]);

    let error = alice_transport
        .dial(bob.id(), &list)
        .await
        .expect_err("nothing is listening");

    match error {
        DialError::Unreachable { attempts } => {
            assert_eq!(attempts.len(), 2, "both candidates should have been tried");
            assert_eq!(
                attempts[0].reachability,
                Reachability::Lan,
                "LAN goes first"
            );
            assert_eq!(attempts[1].reachability, Reachability::Tailnet);
        }
        other => panic!("expected Unreachable, got {other:?}"),
    }
    assert!(
        DialError::Unreachable { attempts: vec![] }.is_retryable(),
        "an unreachable peer is worth retrying"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blob_travels_on_its_own_stream_while_control_messages_keep_flowing() {
    let (alice, bob, alice_transport, bob_transport, bob_addr) = paired_pair().await;

    let blob: Vec<u8> = (0..400_000u32).map(|i| (i % 251) as u8).collect();
    let expected = blob.clone();

    let server = tokio::spawn({
        let bob_transport = Arc::clone(&bob_transport);
        async move {
            let mut link = bob_transport.accept_session().await.unwrap().unwrap();
            // A control message first, then the blob, then another control
            // message — proving the main stream is not blocked behind it.
            let before = link.recv().await.unwrap();
            let received = link.recv_blob().await.unwrap();
            let after = link.recv().await.unwrap();
            (before, received, after)
        }
    });

    let mut link = alice_transport
        .dial(bob.id(), &candidates(bob_addr))
        .await
        .unwrap();

    let before = hello(alice.id());
    link.send(&before).await.unwrap();
    link.send_blob(&blob).await.unwrap();
    let after = SyncMessage::Ack {
        hlc: Hlc::new(1, 0, alice.id()),
    };
    link.send(&after).await.unwrap();

    let (got_before, got_blob, got_after) = tokio::time::timeout(Duration::from_secs(20), server)
        .await
        .expect("blob transfer timed out")
        .expect("server task panicked");

    assert_eq!(got_before, before);
    assert_eq!(
        got_blob.len(),
        expected.len(),
        "blob length changed in flight"
    );
    assert_eq!(got_blob, expected, "blob bytes changed in flight");
    assert_eq!(got_after, after);

    link.close("done");
}

#[tokio::test(flavor = "multi_thread")]
async fn two_strangers_pair_over_the_wire_and_can_then_sync() {
    use clipse_crypto::{
        CandidateAddress, PairingAccept, PairingConfirm, PairingInitiator, PairingResponder,
    };
    use clipse_net::Inbound;

    // Neither device has heard of the other. This is first contact.
    let alice = Device::new();
    let bob = Device::new();
    let alice_transport = Arc::new(alice.transport());
    let bob_transport = Arc::new(bob.transport());
    let alice_addr = alice_transport.local_addr();

    // Alice shows a QR code.
    let initiator = PairingInitiator::create(
        &alice.identity,
        "alice".into(),
        Platform::Windows,
        vec![CandidateAddress::Lan(alice_addr)],
        0,
    );
    let uri = initiator.to_uri();

    // Alice waits for whoever scans it.
    let alice_side = tokio::spawn({
        let alice_transport = Arc::clone(&alice_transport);
        async move {
            let inbound = alice_transport.accept().await.unwrap().unwrap();
            let Inbound::Pairing(exchange) = inbound else {
                panic!("a pairing attempt must not arrive as a session");
            };

            let accept = PairingAccept::from_bytes(exchange.accept_bytes()).unwrap();
            let (confirm, sas, paired_bob) = initiator.accept(&accept, 0).unwrap();
            exchange.confirm(&confirm.to_bytes()).await.unwrap();
            (sas, paired_bob)
        }
    });

    // Bob scans it and answers over the address the QR carried.
    let (responder, accept) = PairingResponder::from_offer(
        &uri,
        &bob.identity,
        "bob".into(),
        Platform::Linux,
        vec![CandidateAddress::Lan(bob_transport.local_addr())],
        0,
    )
    .unwrap();

    let confirm_bytes = bob_transport
        .send_pairing_accept(alice_addr, &accept.to_bytes())
        .await
        .expect("pairing exchange should complete");

    let confirm = PairingConfirm::from_bytes(&confirm_bytes).unwrap();
    let (sas_bob, paired_alice) = responder.verify(&confirm, 0).unwrap();
    let (sas_alice, paired_bob) = alice_side.await.unwrap();

    // The user compares these two screens. They must match, or pairing is off.
    assert_eq!(
        sas_alice, sas_bob,
        "the six digits must agree or the user would refuse"
    );
    assert_eq!(paired_bob.device_id, bob.id());
    assert_eq!(paired_alice.device_id, alice.id());

    // The user confirms on both devices, which is what commits the trust.
    alice.trust.write().unwrap().add_peer(paired_bob);
    bob.trust.write().unwrap().add_peer(paired_alice);

    // And now — the point of all of it — they can hold a sync session.
    let server = tokio::spawn({
        let alice_transport = Arc::clone(&alice_transport);
        async move {
            let mut link = alice_transport.accept_session().await.unwrap().unwrap();
            let first = link.recv().await.unwrap();
            link.send(&first).await.unwrap();
            link
        }
    });

    let mut link = bob_transport
        .dial(alice.id(), &candidates(alice_addr))
        .await
        .expect("devices that just paired must be able to connect");

    let hello = hello(bob.id());
    link.send(&hello).await.unwrap();
    assert_eq!(link.recv().await.unwrap(), hello);

    link.close("done");
    let _ = server.await;
}

/// The bug that made every inbound session between two real machines look
/// broken: the side that speaks last hung up on the very next line, and QUIC
/// threw away the message it had just written.
///
/// The peer here does what a responder does — finishes its own work before
/// reading the last message — which is exactly the window the abrupt close
/// used to land in.
#[tokio::test(flavor = "multi_thread")]
async fn the_last_message_survives_the_sender_hanging_up() {
    let alice = Device::new();
    let bob = Device::new();
    alice.trusts(&bob, "bob");
    bob.trusts(&alice, "alice");

    let alice_transport = Arc::new(alice.transport());
    let bob_transport = Arc::new(bob.transport());
    let bob_addr = bob_transport.local_addr();

    let server = tokio::spawn({
        let bob_transport = Arc::clone(&bob_transport);
        async move {
            let mut link = bob_transport.accept_session().await.unwrap().unwrap();
            // Busy for a moment — writing to a store, say — and only then
            // reading what the dialler left for it.
            tokio::time::sleep(Duration::from_millis(150)).await;
            link.recv().await
        }
    });

    let mut link = alice_transport
        .dial(bob.id(), &candidates(bob_addr))
        .await
        .expect("dial");

    let last = SyncMessage::Ack {
        hlc: Hlc::new(42, 0, alice.id()),
    };
    link.send(&last).await.unwrap();
    link.close_gracefully("done").await;

    let received = server.await.unwrap();
    assert_eq!(
        received.expect("the closing Ack never arrived"),
        last,
        "the peer would have reported a completed session as a failure"
    );
}
