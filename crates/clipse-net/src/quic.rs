//! QUIC transport.
//!
//! # Why there are two layers of encryption
//!
//! QUIC requires TLS, so there is a per-device self-signed certificate — but
//! **TLS is not the authentication boundary here and its certificates are not
//! checked**. The identity Clipse cares about is the one the user confirmed
//! during pairing, and no certificate presented by a socket can prove that.
//! So the TLS layer accepts any peer certificate, and a **Noise_IK handshake**
//! runs on the first bidirectional stream, where a static key that is not in
//! the [`Trust`] set is refused. Yes, that encrypts twice; the cost is
//! irrelevant next to getting trust right. See `docs/sync-protocol.md` §3.
//!
//! # Streams
//!
//! One long-lived bidirectional stream carries the Noise session and every
//! control message. Blobs go on their own unidirectional streams so a 40 MB
//! image cannot stall the control traffic behind it.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use clipse_core::DeviceId;
use clipse_crypto::{DeviceIdentity, HandshakeInitiator, HandshakeResponder, Session, Trust};
use clipse_sync::SyncMessage;
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tracing::{debug, warn};

use crate::candidate::{CandidateList, Reachability};
use crate::framing::{self, Decoder};
use crate::transport::{AttemptFailure, DialError, LinkError, LinkInfo};

/// ALPN identifier for an authenticated sync session. Bumping this is another
/// way a protocol change can be made visible, alongside
/// `clipse_core::PROTOCOL_VERSION`.
const ALPN: &[u8] = b"clipse/1";

/// ALPN for the pairing ceremony.
///
/// Pairing is the one exchange that cannot be authenticated — it is what
/// *creates* the authentication — so it must not arrive on the same path as a
/// session and be mistaken for one. A separate ALPN makes the accept loop
/// branch on what the connection is *for* before it reads a byte, rather than
/// inferring it from the first message.
const PAIRING_ALPN: &[u8] = b"clipse-pair/1";

/// What arrived on the listening endpoint.
pub enum Inbound {
    /// A paired peer completing a Noise handshake.
    Session(PeerLink),
    /// Someone who scanned our QR code. Unauthenticated by definition; the
    /// six-digit code the user compares is what makes it safe.
    Pairing(PairingExchange),
}

/// The responder's side of one pairing ceremony, mid-flight.
pub struct PairingExchange {
    send: SendStream,
    /// Held only so the stream stays open until `confirm` has flushed;
    /// nothing further is read from it.
    _recv: RecvStream,
    connection: Connection,
    accept_bytes: Vec<u8>,
}

impl PairingExchange {
    /// The `PairingAccept` the far side sent. Feed it to
    /// `PairingInitiator::accept`.
    pub fn accept_bytes(&self) -> &[u8] {
        &self.accept_bytes
    }

    pub fn remote_addr(&self) -> SocketAddr {
        self.connection.remote_address()
    }

    /// Send the `PairingConfirm` back and finish.
    pub async fn confirm(mut self, confirm_bytes: &[u8]) -> Result<(), LinkError> {
        write_frame(&mut self.send, confirm_bytes).await?;
        // Give QUIC a moment to flush before the connection drops; a pairing
        // ceremony that loses its last message leaves the two devices
        // disagreeing about whether they are paired.
        let _ = self.send.finish();
        self.connection.closed().await;
        Ok(())
    }

    /// Refuse, without saying why. A stranger probing for a pairing window
    /// learns nothing from the difference between "expired" and "rejected".
    pub fn reject(self) {
        self.connection.close(0u32.into(), b"pairing refused");
    }
}

/// How long to wait for one candidate address before moving to the next.
/// Short: the whole point of the ordered list is to fail over quickly.
const DIAL_TIMEOUT: Duration = Duration::from_secs(3);

/// Ceiling on one wire frame. A frame is one Noise message, which the protocol
/// caps at 65535 anyway; this refuses a bad length prefix before allocating.
const MAX_FRAME_BYTES: u32 = 65_535;

#[derive(Debug, thiserror::Error)]
pub enum QuicError {
    #[error("could not bind a UDP socket: {0}")]
    Bind(#[source] std::io::Error),

    #[error("could not build the TLS configuration: {0}")]
    Tls(String),

    #[error("could not generate a device certificate: {0}")]
    Certificate(String),

    #[error(transparent)]
    Link(#[from] LinkError),
}

/// A QUIC endpoint that both dials and accepts.
pub struct QuicTransport {
    endpoint: Endpoint,
    identity: Arc<DeviceIdentity>,
    trust: Arc<RwLock<Trust>>,
    /// Requests the pairing ALPN rather than the session one.
    pairing_client: quinn::ClientConfig,
}

impl QuicTransport {
    /// Bind on `addr`. Use port 0 to let the OS choose, then read
    /// [`Self::local_addr`] — which is what the mDNS advertisement needs.
    pub fn bind(
        addr: SocketAddr,
        identity: Arc<DeviceIdentity>,
        trust: Arc<RwLock<Trust>>,
    ) -> Result<Self, QuicError> {
        install_crypto_provider();

        let (certificate, key) = self_signed_certificate()?;
        let server = server_config(certificate, key)?;
        let mut endpoint = Endpoint::server(server, addr).map_err(QuicError::Bind)?;
        endpoint.set_default_client_config(client_config(ALPN)?);
        let pairing_client = client_config(PAIRING_ALPN)?;

        Ok(Self {
            endpoint,
            identity,
            trust,
            pairing_client,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.endpoint
            .local_addr()
            .expect("a bound endpoint always has a local address")
    }

    /// Try each candidate in order and complete a Noise handshake on the first
    /// address that answers.
    pub async fn dial(
        &self,
        peer: DeviceId,
        candidates: &CandidateList,
    ) -> Result<PeerLink, DialError> {
        if !self.trust.read().expect(TRUST_POISONED).is_paired(&peer) {
            return Err(DialError::NotPaired);
        }
        if candidates.is_empty() {
            return Err(DialError::NoCandidates);
        }

        let mut attempts = Vec::new();

        for candidate in candidates.dial_order() {
            let connecting = match self.endpoint.connect(candidate.addr, "clipse") {
                Ok(connecting) => connecting,
                Err(e) => {
                    attempts.push(failure(candidate.addr, candidate.reachability, e));
                    continue;
                }
            };

            let connection = match tokio::time::timeout(DIAL_TIMEOUT, connecting).await {
                Ok(Ok(connection)) => connection,
                Ok(Err(e)) => {
                    attempts.push(failure(candidate.addr, candidate.reachability, e));
                    continue;
                }
                Err(_) => {
                    attempts.push(AttemptFailure {
                        addr: candidate.addr,
                        reachability: candidate.reachability,
                        reason: "timed out".into(),
                    });
                    continue;
                }
            };

            // Reached the far side. From here a failure is about *identity*,
            // not reachability, so it must not be reported as another dead
            // address to retry — see `DialError::is_retryable`.
            return self
                .handshake_as_initiator(connection, peer, candidate.addr, candidate.reachability)
                .await;
        }

        Err(DialError::Unreachable { attempts })
    }

    async fn handshake_as_initiator(
        &self,
        connection: Connection,
        peer: DeviceId,
        addr: SocketAddr,
        reachability: Reachability,
    ) -> Result<PeerLink, DialError> {
        // The trust guard is scoped tightly: it must not be held across an
        // await, and the handshake below does I/O.
        let (initiator, first_message) = {
            let trust = self.trust.read().expect(TRUST_POISONED);
            HandshakeInitiator::start(&self.identity, &trust, peer)
                .map_err(|source| DialError::Rejected { source })?
        };

        let unreachable = |e: LinkError| DialError::Unreachable {
            attempts: vec![AttemptFailure {
                addr,
                reachability,
                reason: e.to_string(),
            }],
        };

        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|e| unreachable(LinkError::Transport(e.to_string())))?;

        write_frame(&mut send, &first_message)
            .await
            .map_err(unreachable)?;
        let reply = read_frame(&mut recv).await.map_err(unreachable)?;

        let session = initiator
            .finish(&reply)
            .map_err(|source| DialError::Rejected { source })?;

        Ok(PeerLink {
            info: LinkInfo {
                device: session.remote_device_id(),
                addr,
                reachability,
            },
            connection,
            send,
            recv,
            session,
            decoder: Decoder::new(),
        })
    }

    /// Accept the next inbound connection, whatever it is for. `None` once the
    /// endpoint is closed.
    pub async fn accept(&self) -> Option<Result<Inbound, LinkError>> {
        let incoming = self.endpoint.accept().await?;
        Some(self.accept_one(incoming).await)
    }

    /// Accept, insisting on a sync session. Convenience for callers that are
    /// not offering pairing right now.
    pub async fn accept_session(&self) -> Option<Result<PeerLink, LinkError>> {
        match self.accept().await? {
            Ok(Inbound::Session(link)) => Some(Ok(link)),
            Ok(Inbound::Pairing(exchange)) => {
                exchange.reject();
                Some(Err(LinkError::Transport(
                    "a pairing attempt arrived while none was open".into(),
                )))
            }
            Err(e) => Some(Err(e)),
        }
    }

    async fn accept_one(&self, incoming: quinn::Incoming) -> Result<Inbound, LinkError> {
        let connection = incoming
            .await
            .map_err(|e| LinkError::Transport(e.to_string()))?;
        let addr = connection.remote_address();

        // Which ALPN the peer negotiated says what this connection is for,
        // before any of its bytes are trusted.
        let is_pairing = connection
            .handshake_data()
            .and_then(|data| {
                data.downcast::<quinn::crypto::rustls::HandshakeData>()
                    .ok()
                    .and_then(|d| d.protocol)
            })
            .is_some_and(|protocol| protocol == PAIRING_ALPN);

        if is_pairing {
            let (send, mut recv) = connection
                .accept_bi()
                .await
                .map_err(|e| LinkError::Transport(e.to_string()))?;
            let accept_bytes = read_frame(&mut recv).await?;
            return Ok(Inbound::Pairing(PairingExchange {
                send,
                _recv: recv,
                connection,
                accept_bytes,
            }));
        }

        let (mut send, mut recv) = connection
            .accept_bi()
            .await
            .map_err(|e| LinkError::Transport(e.to_string()))?;

        let first = read_frame(&mut recv).await?;

        let (session, reply) = {
            let trust = self.trust.read().expect(TRUST_POISONED);
            let responder = HandshakeResponder::accept(&self.identity, &trust, &first)?;
            responder.respond()?
        };

        write_frame(&mut send, &reply).await?;

        // Inbound links are LAN by definition of how they arrived; the label
        // is refined later if discovery says otherwise.
        Ok(Inbound::Session(PeerLink {
            info: LinkInfo {
                device: session.remote_device_id(),
                addr,
                reachability: Reachability::Lan,
            },
            connection,
            send,
            recv,
            session,
            decoder: Decoder::new(),
        }))
    }

    /// Run the initiator-facing half of a pairing ceremony: send our
    /// `PairingAccept` to the address from the QR code and wait for the
    /// `PairingConfirm`.
    ///
    /// Unauthenticated on purpose — this is the exchange that establishes
    /// authentication. What makes it safe is the six digits the user compares
    /// afterwards, not anything on this connection.
    pub async fn send_pairing_accept(
        &self,
        addr: SocketAddr,
        accept_bytes: &[u8],
    ) -> Result<Vec<u8>, LinkError> {
        let connection = tokio::time::timeout(
            DIAL_TIMEOUT,
            self.endpoint
                .connect_with(self.pairing_client.clone(), addr, "clipse")
                .map_err(|e| LinkError::Transport(e.to_string()))?,
        )
        .await
        .map_err(|_| LinkError::Transport("pairing dial timed out".into()))?
        .map_err(|e| LinkError::Transport(e.to_string()))?;

        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|e| LinkError::Transport(e.to_string()))?;

        write_frame(&mut send, accept_bytes).await?;
        let confirm = read_frame(&mut recv).await?;
        connection.close(0u32.into(), b"paired");
        Ok(confirm)
    }

    /// Stop accepting and let in-flight connections drain.
    pub fn close(&self) {
        self.endpoint.close(0u32.into(), b"shutting down");
    }
}

const TRUST_POISONED: &str = "trust set poisoned by an earlier panic";

fn failure(
    addr: SocketAddr,
    reachability: Reachability,
    e: impl std::fmt::Display,
) -> AttemptFailure {
    AttemptFailure {
        addr,
        reachability,
        reason: e.to_string(),
    }
}

/// One authenticated, encrypted session with one peer.
pub struct PeerLink {
    info: LinkInfo,
    connection: Connection,
    send: SendStream,
    recv: RecvStream,
    session: Session,
    decoder: Decoder,
}

impl std::fmt::Debug for PeerLink {
    // Manual rather than derived: a derived impl would print the `Session`,
    // and nothing holding key material should be printable by accident.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerLink")
            .field("device", &self.info.device)
            .field("addr", &self.info.addr)
            .field("reachability", &self.info.reachability)
            .finish_non_exhaustive()
    }
}

impl PeerLink {
    pub fn info(&self) -> &LinkInfo {
        &self.info
    }

    pub fn remote_device(&self) -> DeviceId {
        self.info.device
    }

    pub fn epoch(&self) -> u64 {
        self.session.epoch()
    }

    pub fn needs_rekey(&self) -> bool {
        self.session.needs_rekey()
    }

    pub async fn send(&mut self, message: &SyncMessage) -> Result<(), LinkError> {
        for frame in framing::encode(&mut self.session, message)? {
            write_frame(&mut self.send, &frame).await?;
        }
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<SyncMessage, LinkError> {
        loop {
            let frame = read_frame(&mut self.recv).await?;
            if let Some(message) = self.decoder.accept(&mut self.session, &frame)? {
                return Ok(message);
            }
        }
    }

    /// Send blob bytes on their own unidirectional stream, so control traffic
    /// on the main stream keeps flowing while a large image transfers.
    ///
    /// The bytes are encrypted with the same session, which means this call
    /// and [`Self::send`] must not interleave from two tasks — the Noise nonce
    /// is a single counter. Callers drive one link from one task.
    pub async fn send_blob(&mut self, bytes: &[u8]) -> Result<(), LinkError> {
        let mut stream = self
            .connection
            .open_uni()
            .await
            .map_err(|e| LinkError::Transport(e.to_string()))?;

        for segment in bytes.chunks(48_000) {
            let sealed = self.session.write_message(segment)?;
            write_frame_to(&mut stream, &sealed).await?;
        }
        stream
            .finish()
            .map_err(|e| LinkError::Transport(e.to_string()))?;
        Ok(())
    }

    /// Receive one blob from the next unidirectional stream.
    pub async fn recv_blob(&mut self) -> Result<Vec<u8>, LinkError> {
        let mut stream = self
            .connection
            .accept_uni()
            .await
            .map_err(|e| LinkError::Transport(e.to_string()))?;

        let mut out = Vec::new();
        loop {
            match read_frame_from(&mut stream).await {
                Ok(frame) => out.extend_from_slice(&self.session.read_message(&frame)?),
                Err(LinkError::Closed) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    pub fn close(self, reason: &str) {
        self.connection.close(0u32.into(), reason.as_bytes());
    }
}

// --- wire framing on a QUIC stream ----------------------------------------

async fn write_frame(stream: &mut SendStream, frame: &[u8]) -> Result<(), LinkError> {
    write_frame_to(stream, frame).await
}

async fn write_frame_to(stream: &mut SendStream, frame: &[u8]) -> Result<(), LinkError> {
    let len = u32::try_from(frame.len()).map_err(|_| LinkError::TooLarge {
        size: frame.len() as u64,
        max: MAX_FRAME_BYTES as u64,
    })?;
    stream
        .write_all(&len.to_le_bytes())
        .await
        .map_err(|e| LinkError::Transport(e.to_string()))?;
    stream
        .write_all(frame)
        .await
        .map_err(|e| LinkError::Transport(e.to_string()))?;
    Ok(())
}

async fn read_frame(stream: &mut RecvStream) -> Result<Vec<u8>, LinkError> {
    read_frame_from(stream).await
}

async fn read_frame_from(stream: &mut RecvStream) -> Result<Vec<u8>, LinkError> {
    let mut len = [0u8; 4];
    match stream.read_exact(&mut len).await {
        Ok(()) => {}
        Err(_) => return Err(LinkError::Closed),
    }

    let size = u32::from_le_bytes(len);
    if size > MAX_FRAME_BYTES {
        return Err(LinkError::TooLarge {
            size: size as u64,
            max: MAX_FRAME_BYTES as u64,
        });
    }

    let mut frame = vec![0u8; size as usize];
    stream
        .read_exact(&mut frame)
        .await
        .map_err(|e| LinkError::Transport(e.to_string()))?;
    Ok(frame)
}

// --- TLS plumbing ----------------------------------------------------------

fn install_crypto_provider() {
    // Idempotent: `install_default` returns Err if one is already installed,
    // which happens whenever a process builds more than one endpoint (every
    // test in this file).
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn self_signed_certificate()
-> Result<(CertificateDer<'static>, PrivatePkcs8KeyDer<'static>), QuicError> {
    let certified = rcgen::generate_simple_self_signed(vec!["clipse".to_string()])
        .map_err(|e| QuicError::Certificate(e.to_string()))?;
    let key = PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
    Ok((certified.cert.der().clone(), key))
}

fn server_config(
    certificate: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
) -> Result<quinn::ServerConfig, QuicError> {
    let mut tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certificate], key.into())
        .map_err(|e| QuicError::Tls(e.to_string()))?;
    tls.alpn_protocols = vec![ALPN.to_vec(), PAIRING_ALPN.to_vec()];

    let quic = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
        .map_err(|e| QuicError::Tls(e.to_string()))?;
    Ok(quinn::ServerConfig::with_crypto(Arc::new(quic)))
}

fn client_config(alpn: &[u8]) -> Result<quinn::ClientConfig, QuicError> {
    let mut tls = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCert))
        .with_no_client_auth();
    tls.alpn_protocols = vec![alpn.to_vec()];

    let quic = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|e| QuicError::Tls(e.to_string()))?;
    Ok(quinn::ClientConfig::new(Arc::new(quic)))
}

/// Accepts every certificate, deliberately.
///
/// This is not a shortcut. Clipse authenticates peers by the static key the
/// user confirmed during pairing, checked inside the Noise handshake that runs
/// on top of this connection. A certificate chain would be answering a
/// different question — "is this host who DNS says it is" — which is not the
/// question Clipse needs answered and which nothing on a LAN could answer
/// anyway. Removing this verifier would not make the product safer; it would
/// break it while leaving the real check exactly where it already is.
#[derive(Debug)]
struct AcceptAnyServerCert;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        // QUIC is TLS 1.3 only; this can never be reached.
        Err(rustls::Error::PeerIncompatible(
            rustls::PeerIncompatible::Tls12NotOffered,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        // The signature itself is still checked — it proves the peer holds the
        // key in the certificate it presented. What is skipped is only whether
        // that certificate chains to an authority, which is meaningless here.
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Logged rather than returned: a peer that fails to authenticate is a fact
/// about the network, and the accept loop must keep serving everyone else.
pub fn log_accept_failure(error: &LinkError) {
    match error {
        LinkError::Crypto(e) => warn!(error = %e, "rejected a peer we do not trust"),
        other => debug!(error = %other, "inbound connection ended"),
    }
}
