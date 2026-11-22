//! Socket, TLS and the literal-aware response reader.
//!
//! The buffer is owned here (not a `BufReader`) so a STARTTLS upgrade can
//! detach the plain socket without losing bytes, and so the literal marker is
//! the only place the protocol pauses.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::super::protocol::{self, Response, Token};
use super::super::{ImapConfig, ImapError};

/// Ceiling for one protocol line. Headers can be long, but a server that never
/// sends CRLF must not grow the buffer forever.
const MAX_LINE_BYTES: usize = 64 * 1024;
/// A larger literal is not drained: the connection is dropped instead, because
/// discarding hundreds of megabytes just to stay in sync is an attack surface.
const MAX_DRAIN_BYTES: u64 = 512 * 1024 * 1024;
/// Read chunk for literals and drains.
const READ_CHUNK: usize = 8 * 1024;

#[derive(Debug)]
enum Stream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for Stream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.as_mut().get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.as_mut().get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.as_mut().get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.as_mut().get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Buffered IMAP transport. The buffer is owned here (not a `BufReader`) so a
/// STARTTLS upgrade can detach the plain socket without losing bytes.
pub(super) struct Connection {
    stream: Option<Stream>,
    buffer: Vec<u8>,
    position: usize,
    max_literal: u64,
}

impl Connection {
    const fn new(stream: Stream, max_literal: u64) -> Self {
        Self {
            stream: Some(stream),
            buffer: Vec::new(),
            position: 0,
            max_literal,
        }
    }

    pub(super) fn close(&mut self) {
        self.stream = None;
        self.buffer.clear();
        self.position = 0;
    }

    /// Read one complete response, joining lines that end in a literal.
    ///
    /// Boxed on purpose: the assembled state machine is large (line buffer,
    /// literal buffer, token vector), and without the box every `async fn`
    /// that awaits a command would carry ~17 KiB of it in its own future —
    /// `clippy::large_future` on every call site, and a real stack cost for
    /// the per-account poller.
    pub(super) fn read_response<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Response, ImapError>> + Send + 'a>> {
        Box::pin(async move {
            let mut tokens = Vec::new();
            loop {
                let line = self.read_line().await?;
                let Some((start, length)) = protocol::literal_marker(&line) else {
                    protocol::tokenize_into(&line, &mut tokens);
                    break;
                };
                protocol::tokenize_into(&line[..start], &mut tokens);
                if length > MAX_DRAIN_BYTES {
                    return Err(ImapError::Protocol(format!(
                        "literal of {length} bytes exceeds the drain ceiling"
                    )));
                }
                if length > self.max_literal {
                    self.drain_literal(length).await?;
                    tokens.push(Token::LiteralTooLarge(length));
                } else {
                    tokens.push(Token::Literal(self.read_literal(length).await?));
                }
            }
            protocol::classify(&tokens).map_err(ImapError::Protocol)
        })
    }

    async fn read_line(&mut self) -> Result<String, ImapError> {
        loop {
            if let Some(end) = find_crlf(&self.buffer[self.position..]) {
                let start = self.position;
                let stop = start + end;
                let line = String::from_utf8_lossy(&self.buffer[start..stop]).into_owned();
                self.position = stop + 2;
                self.compact();
                return Ok(line);
            }
            if self.buffer.len() - self.position > MAX_LINE_BYTES {
                return Err(ImapError::Protocol(
                    "response line exceeds the 64 KiB ceiling".to_string(),
                ));
            }
            if self.fill().await? == 0 {
                return Err(ImapError::Protocol(
                    "server closed the connection mid-response".to_string(),
                ));
            }
        }
    }

    async fn read_literal(&mut self, length: u64) -> Result<Vec<u8>, ImapError> {
        let mut out = Vec::with_capacity(usize::try_from(length).unwrap_or(READ_CHUNK));
        let mut remaining = length;
        while remaining > 0 {
            let buffered = self.buffer.len() - self.position;
            if buffered > 0 {
                let take = usize::try_from(remaining)
                    .unwrap_or(usize::MAX)
                    .min(buffered);
                out.extend_from_slice(&self.buffer[self.position..self.position + take]);
                self.position += take;
                remaining -= u64::try_from(take).unwrap_or(remaining);
                continue;
            }
            if self.fill().await? == 0 {
                return Err(ImapError::Protocol(
                    "server closed the connection inside a literal".to_string(),
                ));
            }
        }
        self.compact();
        Ok(out)
    }

    async fn drain_literal(&mut self, length: u64) -> Result<(), ImapError> {
        let mut remaining = length;
        while remaining > 0 {
            let buffered = self.buffer.len() - self.position;
            if buffered > 0 {
                let take = usize::try_from(remaining)
                    .unwrap_or(usize::MAX)
                    .min(buffered);
                self.position += take;
                remaining -= u64::try_from(take).unwrap_or(remaining);
                continue;
            }
            if self.fill().await? == 0 {
                return Err(ImapError::Protocol(
                    "server closed the connection inside a literal".to_string(),
                ));
            }
        }
        self.compact();
        Ok(())
    }

    /// Read more bytes into the buffer. Returns how many arrived.
    async fn fill(&mut self) -> Result<usize, ImapError> {
        let Some(stream) = self.stream.as_mut() else {
            return Err(ImapError::Protocol("connection is closed".to_string()));
        };
        let mut chunk = [0_u8; READ_CHUNK];
        let read = stream.read(&mut chunk).await?;
        self.buffer.extend_from_slice(&chunk[..read]);
        Ok(read)
    }

    fn compact(&mut self) {
        if self.position == self.buffer.len() {
            self.buffer.clear();
            self.position = 0;
        } else if self.position > READ_CHUNK {
            self.buffer.drain(..self.position);
            self.position = 0;
        }
    }

    pub(super) async fn write_all(&mut self, bytes: &[u8]) -> Result<(), ImapError> {
        let Some(stream) = self.stream.as_mut() else {
            return Err(ImapError::Protocol("connection is closed".to_string()));
        };
        stream.write_all(bytes).await?;
        stream.flush().await?;
        Ok(())
    }

    /// Replace the clear-text socket with a TLS one. Refuses when the server
    /// sent bytes past the `STARTTLS` acknowledgment: those would be plaintext
    /// injected ahead of the handshake.
    pub(super) async fn upgrade(&mut self, host: &str) -> Result<(), ImapError> {
        if self.position != self.buffer.len() {
            return Err(ImapError::Protocol(
                "server sent data before the STARTTLS handshake".to_string(),
            ));
        }
        self.buffer.clear();
        self.position = 0;
        let stream = self
            .stream
            .take()
            .ok_or_else(|| ImapError::Protocol("connection is closed".to_string()))?;
        let Stream::Plain(tcp) = stream else {
            return Err(ImapError::Protocol("STARTTLS requested twice".to_string()));
        };
        let connector = TlsConnector::from(tls_config()?);
        let server_name = server_name(host)?;
        let tls = connector
            .connect(server_name, tcp)
            .await
            .map_err(|e| ImapError::Tls(format!("{host}: {e}")))?;
        self.stream = Some(Stream::Tls(Box::new(tls)));
        Ok(())
    }
}

/// Open the socket (implicit TLS when configured) and wrap it in a buffered
/// connection. STARTTLS is a command-level upgrade, performed by the session
/// after the greeting.
pub(super) async fn open(config: &ImapConfig) -> Result<Connection, ImapError> {
    let seconds = config.timeout_secs.max(1);
    let stream = tokio::time::timeout(
        Duration::from_secs(seconds),
        TcpStream::connect((config.host.as_str(), config.port)),
    )
    .await
    .map_err(|_| ImapError::Timeout(seconds))?
    .map_err(|source| ImapError::Connect {
        host: config.host.clone(),
        port: config.port,
        source,
    })?;
    stream.set_nodelay(true)?;

    let stream = if config.use_tls {
        let connector = TlsConnector::from(tls_config()?);
        let name = server_name(&config.host)?;
        let tls = tokio::time::timeout(
            Duration::from_secs(seconds),
            connector.connect(name, stream),
        )
        .await
        .map_err(|_| ImapError::Timeout(seconds))?
        .map_err(|e| ImapError::Tls(format!("{}: {e}", config.host)))?;
        Stream::Tls(Box::new(tls))
    } else {
        Stream::Plain(stream)
    };
    Ok(Connection::new(stream, config.max_message_bytes))
}

fn find_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|window| window == b"\r\n")
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, ImapError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| ImapError::Tls(format!("client configuration: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

fn server_name(host: &str) -> Result<ServerName<'static>, ImapError> {
    ServerName::try_from(host.to_string())
        .map_err(|e| ImapError::Config(format!("`{host}` is not a TLS server name: {e}")))
}

/// Quote a mailbox or credential for an IMAP command.
pub(super) fn quote(value: &str) -> Result<String, ImapError> {
    if value.chars().any(|c| matches!(c, '\r' | '\n' | '\0')) {
        return Err(ImapError::Config(
            "a command argument contains a control character".to_string(),
        ));
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// Apply the configured timeout to one protocol step.
pub(super) async fn timed<F, T>(seconds: u64, future: F) -> Result<T, ImapError>
where
    F: Future<Output = Result<T, ImapError>>,
{
    let seconds = seconds.max(1);
    tokio::time::timeout(Duration::from_secs(seconds), future)
        .await
        .map_err(|_| ImapError::Timeout(seconds))?
}
