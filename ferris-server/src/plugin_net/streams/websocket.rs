//! The outbound WebSocket client behind `io::connect-stream`.
//!
//! The handshake is RFC 6455; `wss` is terminated by `tokio-rustls` with the
//! app's `tls-client` policy, so `verify`, `server-name` and ALPN mean exactly
//! what they mean on `io::request` over `https`.

use anyhow::{Context, Result};
use ferris_wasm::host::BridgeError;
use ferris_wasm::{Capability, OutboundStreamSpec, OutboundTls, StreamInfo, TlsProtocol};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request as WebSocketRequest;

use super::io::websocket_stream;
use super::raw::{connect_first, limits_of, with_peer};
use super::{Limits, ServerStreams};
use crate::plugin_net::egress::EgressPolicy;

impl ServerStreams {
    /// Complete a WebSocket handshake over an established transport and pump it.
    ///
    /// Generic over the transport so `ws` (a `TcpStream`) and `wss` (a
    /// `tokio-rustls` stream) share the handshake, the id and the pump without an
    /// `enum` wrapper or a boxed trait object.
    pub(super) async fn open_websocket<S>(
        &self,
        app: &str,
        request: WebSocketRequest,
        stream: S,
        info: StreamInfo,
        limits: Limits,
    ) -> Result<String>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (socket, response) = tokio_tungstenite::client_async(request, stream)
            .await
            .context("the websocket handshake")?;
        tracing::debug!(
            app = %app,
            status = response.status().as_u16(),
            protocol = ?response.headers().get("sec-websocket-protocol"),
            "outbound websocket accepted"
        );
        let id = format!("stream-{}", uuid::Uuid::new_v4());
        let (source, sink) = websocket_stream(socket);
        self.core
            .start_stream(app, &id, info, source, sink, limits, None);
        Ok(id)
    }

    /// `io::connect-stream` for `ws://`/`wss://`.
    pub(super) async fn connect_websocket(
        &self,
        app: &str,
        spec: &OutboundStreamSpec,
        unrestricted: bool,
        allow_private: bool,
    ) -> Result<String> {
        let (host, port, secure) = parse_ws_uri(&spec.uri)?;
        if !secure && spec.tls.is_some() {
            return Err(BridgeError::unsupported(
                "io::connect-stream: `ws://` is cleartext, so a `tls-client` policy cannot \
                 apply; use `wss://`",
            ));
        }
        let scheme = if secure { "wss" } else { "ws" };
        let limits = limits_of(spec);
        let policy = EgressPolicy::from_env(unrestricted, allow_private);
        let addresses = policy.resolve(&host, port).await?;
        // Built before the socket so a malformed URI fails without a connection.
        let request = spec
            .uri
            .as_str()
            .into_client_request()
            .with_context(|| format!("`{}` is not a websocket request", spec.uri))?;
        let tcp = connect_first(&addresses)
            .await
            .with_context(|| format!("opening {scheme}://{host}:{port}"))?;
        let peer = tcp
            .peer_addr()
            .map_or_else(|_| format!("{host}:{port}"), |addr| addr.to_string());
        let info = with_peer(StreamInfo::new("websocket"), peer);

        if !secure {
            return self.open_websocket(app, request, tcp, info, limits).await;
        }

        // A declared `tls-client` is material the app presents or a trust policy
        // it narrows, so it needs the grant; a bare `wss://` keeps the host's
        // default trust store, exactly like `https://` over `io::request`.
        if spec.tls.is_some() && !self.granted(app, Capability::TlsClient) {
            return Err(BridgeError::unsupported(
                "io::connect-stream: a `wss` stream with a `tls-client` policy needs the \
                 `tls-client` capability",
            ));
        }
        let outbound = match spec.tls.as_ref() {
            Some(tls) => self.client_tls(app, tls).await?,
            // A bare `wss://` verifies against the host's trust store. Written
            // out instead of `OutboundTls::default()` on purpose: the derived
            // default has `verify: false`, which would accept any certificate —
            // the opposite of what "no policy declared" must mean.
            None => OutboundTls {
                cert: None,
                key: None,
                ca: None,
                server_name: None,
                alpn: Vec::new(),
                min_version: TlsProtocol::Tls12,
                verify: true,
            },
        };
        let config = ferris_wasm::tls::build_client_config(&outbound)?;
        // The policy's `server-name` wins over the URI host: a stream pinned to a
        // name behind a shared address would otherwise verify against the wrong
        // identity.
        let name = ferris_wasm::tls::server_name(Some(&outbound), &host)?;
        let connector = tokio_rustls::TlsConnector::from(config);
        let stream = connector
            .connect(name, tcp)
            .await
            .with_context(|| format!("TLS handshake with {host}:{port}"))?;
        self.open_websocket(app, request, stream, info, limits)
            .await
    }
}

/// Split a `ws://`/`wss://` URI into the authority the client dials and whether
/// the scheme asks for TLS.
///
/// Only the authority is taken from here: the handshake request is built from
/// the URI as declared, so the guest's path, query and `Host` header survive
/// verbatim.
pub(super) fn parse_ws_uri(raw: &str) -> Result<(String, u16, bool)> {
    let (rest, secure) = if let Some(rest) = raw.strip_prefix("wss://") {
        (rest, true)
    } else if let Some(rest) = raw.strip_prefix("ws://") {
        (rest, false)
    } else {
        anyhow::bail!(
            "`{raw}` must start with `ws://` or `wss://` for a websocket stream (`tcp`/`tls` \
             carry no HTTP handshake)"
        );
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let (host, port) = host_and_port(authority, if secure { 443 } else { 80 })?;
    Ok((host, port, secure))
}

/// `host[:port]` with a default port; IPv6 literals are bracketed.
pub(super) fn host_and_port(authority: &str, default_port: u16) -> Result<(String, u16)> {
    if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, tail) = bracketed
            .split_once(']')
            .ok_or_else(|| anyhow::anyhow!("`{authority}` has an unterminated IPv6 literal"))?;
        let port = match tail.strip_prefix(':') {
            Some(port) => port
                .parse::<u16>()
                .with_context(|| format!("`{authority}` has an invalid port"))?,
            None if tail.is_empty() => default_port,
            None => anyhow::bail!("`{authority}` has trailing characters after the IPv6 literal"),
        };
        return Ok((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            if host.is_empty() {
                anyhow::bail!("`{authority}` has an empty host");
            }
            let port = port
                .parse::<u16>()
                .with_context(|| format!("`{authority}` has an invalid port"))?;
            Ok((host.to_string(), port))
        }
        None if !authority.is_empty() => Ok((authority.to_string(), default_port)),
        None => anyhow::bail!("a websocket URI needs a host"),
    }
}
