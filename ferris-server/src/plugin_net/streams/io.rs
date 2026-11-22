//! The two halves every transport implements.
//!
//! The pump polls a read half and a write half at the same time, and
//! `tokio::select!` cannot hold two `&mut` borrows of one value — hence two
//! traits instead of one. `tcp`/`tls` wrap the halves of an `AsyncRead +
//! AsyncWrite` stream; `ws`/`wss` wrap the `Stream`/`Sink` pair of a
//! `WebSocketStream`, which is the same shape.

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

/// The reading half of one stream, whatever the wire protocol is.
///
/// The pump owns the two halves separately because it polls both at once: a
/// single value would be borrowed twice by `tokio::select!`. `next` returns
/// `Ok(None)` for a clean end of stream (EOF, a WebSocket Close frame) and
/// `Err` for a failure that must be reported as `error`.
#[async_trait]
pub(super) trait ChunkSource: Send {
    /// The next chunk the peer sent, or `None` at a clean end of stream.
    async fn next(&mut self) -> Result<Option<Vec<u8>>>;
}

/// The writing half of one stream, whatever the wire protocol is.
#[async_trait]
pub(super) trait ChunkSink: Send {
    /// Write one chunk to the peer, awaiting the socket (backpressure).
    async fn write(&mut self, bytes: &[u8]) -> Result<()>;
    /// Best-effort graceful close: flush the wire, then announce the end.
    async fn close(&mut self);
}

/// The reading half of a raw byte stream (`tcp`/`tls`).
pub(super) struct ByteSource<S> {
    reader: tokio::io::ReadHalf<S>,
    /// Reused between reads so a steady stream does not allocate per chunk.
    buffer: Vec<u8>,
}

#[async_trait]
impl<S> ChunkSource for ByteSource<S>
where
    S: AsyncRead + AsyncWrite + Send,
{
    async fn next(&mut self) -> Result<Option<Vec<u8>>> {
        let read = self
            .reader
            .read(&mut self.buffer)
            .await
            .context("reading from the peer")?;
        if read == 0 {
            return Ok(None);
        }
        Ok(Some(self.buffer[..read].to_vec()))
    }
}

/// The writing half of a raw byte stream (`tcp`/`tls`).
pub(super) struct ByteSink<S> {
    writer: tokio::io::WriteHalf<S>,
}

#[async_trait]
impl<S> ChunkSink for ByteSink<S>
where
    S: AsyncRead + AsyncWrite + Send,
{
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer
            .write_all(bytes)
            .await
            .context("writing to the peer")
    }

    async fn close(&mut self) {
        let _ = self.writer.shutdown().await;
    }
}

/// Split a raw byte stream into the pair the pump drives.
pub(super) fn byte_stream<S>(stream: S, max_frame: usize) -> (ByteSource<S>, ByteSink<S>)
where
    S: AsyncRead + AsyncWrite,
{
    let (reader, writer) = tokio::io::split(stream);
    (
        ByteSource {
            reader,
            buffer: vec![0u8; max_frame.max(1)],
        },
        ByteSink { writer },
    )
}

/// The reading half of a WebSocket connection.
///
/// Text and binary frames are the peer's chunks. Ping/Pong are protocol
/// housekeeping (tungstenite answers a Ping itself) and never reach the guest,
/// which is why the guest cannot observe them as data.
pub(super) struct WsSource<S> {
    stream: SplitStream<WebSocketStream<S>>,
}

#[async_trait]
impl<S> ChunkSource for WsSource<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn next(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            let Some(message) = self.stream.next().await else {
                return Ok(None);
            };
            match message.context("reading a websocket frame")? {
                Message::Text(text) => return Ok(Some(text.as_str().as_bytes().to_vec())),
                Message::Binary(bytes) => return Ok(Some(bytes.to_vec())),
                Message::Close(_) => return Ok(None),
                // Ping/Pong are the protocol's (`tungstenite` answers a Ping
                // itself) and a raw frame is not a message this layer creates.
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    }
}

/// The writing half of a WebSocket connection.
pub(super) struct WsSink<S> {
    sink: SplitSink<WebSocketStream<S>, Message>,
}

#[async_trait]
impl<S> ChunkSink for WsSink<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        // `SinkExt::send` flushes into the socket, so a slow peer backs the
        // guest's `io::send-stream` up instead of growing a buffer here.
        self.sink
            .send(Message::binary(bytes.to_vec()))
            .await
            .context("writing a websocket frame")
    }

    async fn close(&mut self) {
        // The Close frame is the protocol's clean shutdown; a peer that already
        // went away makes the write fail, which is not an error of ours.
        let _ = self.sink.close().await;
    }
}

/// Split a completed WebSocket handshake into the pair the pump drives.
pub(super) fn websocket_stream<S>(socket: WebSocketStream<S>) -> (WsSource<S>, WsSink<S>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (sink, stream) = socket.split();
    (WsSource { stream }, WsSink { sink })
}
