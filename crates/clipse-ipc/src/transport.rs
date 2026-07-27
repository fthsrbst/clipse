//! Local transport: a unix socket, or a named pipe on Windows.
//!
//! Both are user-scoped by the OS — a unix socket sits in the user's data
//! directory with default permissions, and a named pipe created by this process
//! inherits its security descriptor. Neither is reachable from the network, and
//! that is deliberate: peer traffic goes over QUIC in `clipse-net`, never here.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// A connected local stream.
pub struct IpcStream(Inner);

impl std::fmt::Debug for IpcStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IpcStream")
    }
}

#[cfg(unix)]
type Inner = tokio::net::UnixStream;

#[cfg(windows)]
enum Inner {
    Server(tokio::net::windows::named_pipe::NamedPipeServer),
    Client(tokio::net::windows::named_pipe::NamedPipeClient),
}

impl AsyncRead for IpcStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        #[cfg(unix)]
        {
            Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
        }
        #[cfg(windows)]
        {
            match &mut self.get_mut().0 {
                Inner::Server(s) => Pin::new(s).poll_read(cx, buf),
                Inner::Client(c) => Pin::new(c).poll_read(cx, buf),
            }
        }
    }
}

impl AsyncWrite for IpcStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        #[cfg(unix)]
        {
            Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
        }
        #[cfg(windows)]
        {
            match &mut self.get_mut().0 {
                Inner::Server(s) => Pin::new(s).poll_write(cx, buf),
                Inner::Client(c) => Pin::new(c).poll_write(cx, buf),
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        #[cfg(unix)]
        {
            Pin::new(&mut self.get_mut().0).poll_flush(cx)
        }
        #[cfg(windows)]
        {
            match &mut self.get_mut().0 {
                Inner::Server(s) => Pin::new(s).poll_flush(cx),
                Inner::Client(c) => Pin::new(c).poll_flush(cx),
            }
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        #[cfg(unix)]
        {
            Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
        }
        #[cfg(windows)]
        {
            match &mut self.get_mut().0 {
                Inner::Server(s) => Pin::new(s).poll_shutdown(cx),
                Inner::Client(c) => Pin::new(c).poll_shutdown(cx),
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("another Clipse daemon is already listening on {endpoint}")]
    AlreadyRunning { endpoint: String },

    #[error("no Clipse daemon is listening on {endpoint}")]
    NotRunning { endpoint: String },

    #[error("io: {0}")]
    Io(#[from] io::Error),
}

/// Accepts client connections on the daemon side.
#[derive(Debug)]
pub struct Listener {
    endpoint: String,
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
    #[cfg(windows)]
    pending: tokio::net::windows::named_pipe::NamedPipeServer,
}

impl Listener {
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    #[cfg(unix)]
    pub async fn bind(endpoint: &str) -> Result<Self, TransportError> {
        use std::path::Path;

        let path = Path::new(endpoint);
        if path.exists() {
            // A socket file left behind by a crashed daemon looks identical to
            // a live one; the only reliable test is to try talking to it.
            if tokio::net::UnixStream::connect(path).await.is_ok() {
                return Err(TransportError::AlreadyRunning { endpoint: endpoint.to_string() });
            }
            std::fs::remove_file(path)?;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let inner = tokio::net::UnixListener::bind(path)?;
        Ok(Self { endpoint: endpoint.to_string(), inner })
    }

    #[cfg(windows)]
    pub async fn bind(endpoint: &str) -> Result<Self, TransportError> {
        use tokio::net::windows::named_pipe::ServerOptions;

        // `first_pipe_instance` makes the OS enforce single-daemon for us:
        // creating a second one fails instead of silently load-balancing
        // clients across two daemons.
        let pending = ServerOptions::new()
            .first_pipe_instance(true)
            .create(endpoint)
            .map_err(|e| {
                if e.raw_os_error() == Some(231) || e.kind() == io::ErrorKind::PermissionDenied {
                    TransportError::AlreadyRunning { endpoint: endpoint.to_string() }
                } else {
                    TransportError::Io(e)
                }
            })?;
        Ok(Self { endpoint: endpoint.to_string(), pending })
    }

    #[cfg(unix)]
    pub async fn accept(&mut self) -> Result<IpcStream, TransportError> {
        let (stream, _addr) = self.inner.accept().await?;
        Ok(IpcStream(stream))
    }

    #[cfg(windows)]
    pub async fn accept(&mut self) -> Result<IpcStream, TransportError> {
        use tokio::net::windows::named_pipe::ServerOptions;

        self.pending.connect().await?;
        // A named pipe instance is consumed by the client that connects to it,
        // so the next instance has to be created before returning this one.
        let next = ServerOptions::new().create(&self.endpoint)?;
        let connected = std::mem::replace(&mut self.pending, next);
        Ok(IpcStream(Inner::Server(connected)))
    }
}

#[cfg(unix)]
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.endpoint);
    }
}

/// Connect to a running daemon.
pub async fn connect(endpoint: &str) -> Result<IpcStream, TransportError> {
    #[cfg(unix)]
    {
        match tokio::net::UnixStream::connect(endpoint).await {
            Ok(s) => Ok(IpcStream(s)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(TransportError::NotRunning { endpoint: endpoint.to_string() })
            }
            Err(e) => Err(e.into()),
        }
    }
    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ClientOptions;

        // ERROR_PIPE_BUSY means every pre-created instance is taken; the
        // daemon makes a new one as soon as it finishes accepting, so a short
        // retry is the documented way to handle it.
        const BUSY: i32 = 231;
        for _ in 0..20 {
            match ClientOptions::new().open(endpoint) {
                Ok(c) => return Ok(IpcStream(Inner::Client(c))),
                Err(e) if e.raw_os_error() == Some(BUSY) => {
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    return Err(TransportError::NotRunning { endpoint: endpoint.to_string() });
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(TransportError::Io(io::Error::new(
            io::ErrorKind::TimedOut,
            "named pipe stayed busy",
        )))
    }
}
