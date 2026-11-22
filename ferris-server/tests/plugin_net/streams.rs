//! Outbound streams: `tcp` and the `ws`/`wss` WebSocket client.

use std::sync::Arc;

use ferris_server::plugin_net::ServerStreams;
use ferris_server::plugin_net::tls_routes;
use ferris_wasm::{ListenerTransport, OutboundPolicy, OutboundStreamSpec, StreamService};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use super::support::*;

/// An outbound stream reaches the peer, `send-stream` writes to it and the app
/// cannot write to another app's stream.
#[tokio::test]
async fn an_outbound_stream_connects_and_writes_to_its_peer() {
    let Some(store) = store().await else { return };
    let transport = ServerStreams::new(store);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener binds");
    let port = listener.local_addr().expect("address").port();
    let accept = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("connection accepted");
        let mut received = vec![0u8; 64];
        let n = socket.read(&mut received).await.expect("read");
        received.truncate(n);
        received
    });

    let id = tokio::time::timeout(
        TIMEOUT,
        transport.connect("probe", &connect_spec(port, true)),
    )
    .await
    .expect("connect returns in time")
    .expect("the loopback connection is allowed");

    transport
        .send("probe", &id, b"hello peer")
        .await
        .expect("the guest may write to its own stream");

    let received = tokio::time::timeout(TIMEOUT, accept)
        .await
        .expect("the peer reads in time")
        .expect("the accept task finished");
    assert_eq!(received, b"hello peer");

    // Another app's id is not the same socket.
    let err = transport
        .send("other", &id, b"x")
        .await
        .expect_err("a foreign app must not write to this stream");
    assert!(err.to_string().contains("does not belong"), "{err}");

    transport.close("probe", &id).await.expect("close succeeds");
    transport
        .close("probe", &id)
        .await
        .expect("closing twice is idempotent for a guest's cleanup path");
}

/// A loopback target needs `allow-private`; the allow-list alone is not enough,
/// because an allow-listed *name* can point at a private address.
#[tokio::test]
async fn a_private_stream_target_needs_the_grant() {
    let Some(store) = store().await else { return };
    let transport = ServerStreams::new(store);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener binds");
    let port = listener.local_addr().expect("address").port();

    let err = transport
        .connect("probe", &connect_spec(port, false))
        .await
        .expect_err("loopback without `allow-private` is refused");
    assert!(err.to_string().contains("private address"), "{err}");
}

/// An outbound WebSocket completes the RFC 6455 handshake against a loopback
/// peer, `send-stream` puts a frame on the wire, and the guest's reply comes
/// back: the whole loop, with no mock of the transport.
#[tokio::test]
async fn an_outbound_websocket_echoes_through_the_guest() {
    let Some(store) = store().await else { return };
    let transport = ServerStreams::new(store);

    let (port, mut received, server) = websocket_echo_server(None).await;
    let id = tokio::time::timeout(TIMEOUT, transport.connect("probe", &ws_spec(port)))
        .await
        .expect("connect returns in time")
        .expect("the loopback websocket handshake succeeds");

    transport
        .send("probe", &id, b"hello ws")
        .await
        .expect("the guest may write to its own stream");

    // 1) the peer received what `send-stream` wrote; 2) the peer's echo came
    // back through `handle-stream` and its return value was relayed on.
    let written = tokio::time::timeout(TIMEOUT, received.recv())
        .await
        .expect("the peer reads in time")
        .expect("the peer saw a frame");
    assert_eq!(written, b"hello ws");
    let echoed = tokio::time::timeout(TIMEOUT, received.recv())
        .await
        .expect("the guest reply returns in time")
        .expect("the guest's echo reached the peer");
    assert_eq!(echoed, b"hello ws");

    transport.close("probe", &id).await.expect("close succeeds");
    server.abort();
}

/// `wss` terminates TLS with the app's `tls-client` policy: the fixture CA is
/// the only trust anchor, the policy's `server-name` is what verifies, and the
/// same stream then behaves like any other WebSocket.
#[tokio::test]
async fn a_wss_stream_uses_the_apps_client_tls_policy() {
    let Some(store) = store().await else { return };
    let transport = ServerStreams::new(Arc::clone(&store));

    let server_config = tls_routes::build_server_config(
        &fixture_text("server.crt"),
        &fixture_text("server.key"),
        None,
        ferris_wasm::ClientAuth::None,
        &["localhost".to_string()],
        &[],
        &ferris_wasm::TlsVersion::Tls12,
    )
    .expect("the fixture material builds a server config");
    let (port, mut received, server) = websocket_echo_server(Some(server_config)).await;

    let mut spec = ws_spec(port);
    spec.uri = format!("wss://localhost:{port}/echo");
    spec.tls = Some(TlsClient {
        cert: None,
        key: None,
        ca: Some(TlsMaterial::Pem(fixture_text("ca.crt"))),
        server_name: Some("localhost".to_string()),
        alpn: Vec::new(),
        min_version: ferris_wasm::TlsVersion::Tls12,
        verify: true,
    });

    let id = tokio::time::timeout(TIMEOUT, transport.connect("probe", &spec))
        .await
        .expect("connect returns in time")
        .expect("the TLS handshake trusts the fixture CA");
    transport
        .send("probe", &id, b"secure ws")
        .await
        .expect("the guest may write to its own stream");
    let written = tokio::time::timeout(TIMEOUT, received.recv())
        .await
        .expect("the peer reads in time")
        .expect("the peer saw a frame");
    assert_eq!(written, b"secure ws");
    let echoed = tokio::time::timeout(TIMEOUT, received.recv())
        .await
        .expect("the guest reply returns in time")
        .expect("the guest's echo reached the peer");
    assert_eq!(echoed, b"secure ws");
    transport.close("probe", &id).await.expect("close succeeds");

    // The policy is enforced, not decorative: without the app's CA the same
    // name does not verify against the host's trust store.
    let mut untrusted = spec.clone();
    untrusted.tls = None;
    let err = transport
        .connect("probe", &untrusted)
        .await
        .expect_err("the fixture CA is not in the host trust store");
    let text = format!("{err:#}").to_lowercase();
    assert!(
        text.contains("certificate") || text.contains("tls") || text.contains("handshake"),
        "unexpected failure shape: {err}"
    );
    server.abort();
}

/// An outbound `ws://` spec pointing at a loopback echo server.
fn ws_spec(port: u16) -> OutboundStreamSpec {
    OutboundStreamSpec {
        transport: ListenerTransport::Websocket,
        uri: format!("ws://127.0.0.1:{port}/echo"),
        address: None,
        port: None,
        tls: None,
        policy: Some(OutboundPolicy {
            allowed_hosts: vec!["127.0.0.1".to_string()],
            allow_private: true,
            max_body: None,
            unrestricted: false,
        }),
        max_frame: None,
        idle_timeout_seconds: Some(5),
    }
}

/// A loopback WebSocket echo server: it records every frame it receives and
/// echoes it back, so a host test can tell "the frame reached the peer" from
/// "the peer's frame reached the guest".
///
/// The TLS variant wraps the accepted socket in the app's server config, which
/// is how the `wss` test exercises the real handshake.
async fn websocket_echo_server(
    tls: Option<Arc<rustls::ServerConfig>>,
) -> (u16, mpsc::UnboundedReceiver<Vec<u8>>, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the echo listener binds");
    let port = listener.local_addr().expect("address").port();
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let recorded = tx.clone();
            let tls = tls.clone();
            tokio::spawn(async move {
                if let Some(config) = tls {
                    let acceptor = tokio_rustls::TlsAcceptor::from(config);
                    if let Ok(stream) = acceptor.accept(socket).await {
                        echo_frames(stream, recorded).await;
                    }
                } else {
                    echo_frames(socket, recorded).await;
                }
            });
        }
    });
    (port, rx, handle)
}

/// The server half of the echo: record, then send the same frame back.
async fn echo_frames<S>(socket: S, recorded: mpsc::UnboundedSender<Vec<u8>>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Ok(mut socket) = accept_async(socket).await else {
        return;
    };
    while let Some(Ok(message)) = socket.next().await {
        match message {
            Message::Text(text) => {
                recorded.send(text.as_str().as_bytes().to_vec()).ok();
                if socket.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
            Message::Binary(bytes) => {
                recorded.send(bytes.to_vec()).ok();
                if socket.send(Message::Binary(bytes)).await.is_err() {
                    break;
                }
            }
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
}
