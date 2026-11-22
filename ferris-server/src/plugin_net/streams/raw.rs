//! `tcp`/`tls` streams: the outbound socket, the listener and the boot-time
//! binding of the listeners an app declared.
//!
//! A raw socket carries no request, so nothing is authenticated at connect
//! time: the guest authenticates its own frames, which is why
//! `io::listen-stream` refuses an `auth` it cannot enforce.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use ferris_wasm::host::BridgeError;
use ferris_wasm::{Capability, ListenerTransport, OutboundStreamSpec, StreamInfo, StreamService};
use tokio::net::TcpStream;

use super::core::TransportCore;
use super::io::byte_stream;
use super::{DEFAULT_IDLE, DEFAULT_MAX_FRAME, Limits, ServerStreams};

/// A raw socket that has been dialled, before its transport wrapper.
///
/// `tcp` and `tls` differ only in that wrapper, so the connect phase hands the
/// same value to both instead of repeating the address bookkeeping.
pub(super) struct Dialled {
    pub(super) stream: TcpStream,
    /// "ip:port" of the peer, for `stream-info`.
    pub(super) peer: String,
    pub(super) host: String,
    pub(super) port: u16,
}

/// Connect to the first reachable address, trying each resolved address once.
pub(super) async fn connect_first(addresses: &[SocketAddr]) -> Result<TcpStream> {
    let mut last = None;
    for address in addresses {
        match TcpStream::connect(address).await {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some((address, e)),
        }
    }
    match last {
        Some((address, error)) => Err(anyhow::anyhow!("{address}: {error}")),
        None => Err(anyhow::anyhow!("no address to connect to")),
    }
}

/// The address a listener binds when the app declares no host: loopback.
/// It used to be `0.0.0.0`, which published every declared listener on every
/// interface the host had (WASM-013/014). A service that must be reachable from
/// another machine declares its address explicitly, so exposing it stays a line
/// in the app's descriptor the operator can read instead of a default nobody
/// wrote.
pub(super) const DEFAULT_BIND_HOST: &str = "127.0.0.1";

/// Connections one declared listener serves at once.
///
/// An accepted connection is a task, a buffer and a guest call; without the cap
/// a peer that opens sockets and never writes them grows all three. Past the cap
/// the socket is closed rather than queued, so the listener keeps serving the
/// connections it already admitted.
const MAX_LISTENER_CONNECTIONS: usize = 128;

/// Resolve a listener bind target from the optional `address`/`port` pair the
/// WIT declares.
///
/// `None`/`0` means "infer it": the host keeps the address the app declared,
/// falls back to [`DEFAULT_BIND_HOST`], and lets the operating system pick an
/// ephemeral port (`0`) that the listener reports through `stream-describe`.
/// An address that already carries `:port` is still honoured, so old
/// declarations keep working.
pub(super) fn bind_authority(address: Option<&str>, port: Option<u16>) -> String {
    let declared = address.map(str::trim).filter(|value| !value.is_empty());
    let (host, embedded) = declared.map_or_else(
        || (DEFAULT_BIND_HOST.to_string(), None),
        |raw| match split_authority(raw, None) {
            Ok((host, port)) => (host, Some(port)),
            Err(_) => (raw.to_string(), None),
        },
    );
    let port = port.filter(|port| *port != 0).or(embedded).unwrap_or(0);
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host
    };
    format!("{host}:{port}")
}

/// Parse `host:port` (IPv6 in brackets) out of a `scheme://` URI or a bare
/// listener address.
pub(super) fn split_authority(raw: &str, scheme: Option<&str>) -> Result<(String, u16)> {
    let rest = match scheme {
        Some(scheme) => raw
            .strip_prefix(scheme)
            .ok_or_else(|| anyhow::anyhow!("`{raw}` must start with `{scheme}`"))?,
        None => raw,
    };
    let rest = rest.trim_end_matches('/');
    if let Some(bracketed) = rest.strip_prefix('[') {
        let (host, tail) = bracketed
            .split_once(']')
            .ok_or_else(|| anyhow::anyhow!("`{raw}` has an unterminated IPv6 literal"))?;
        let port = tail
            .strip_prefix(':')
            .and_then(|port| port.parse::<u16>().ok())
            .ok_or_else(|| anyhow::anyhow!("`{raw}` has no port"))?;
        return Ok((host.to_string(), port));
    }
    let (host, port) = rest
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("`{raw}` has no port (`host:port`)"))?;
    if host.is_empty() {
        anyhow::bail!("`{raw}` has an empty host");
    }
    let port = port
        .parse::<u16>()
        .with_context(|| format!("`{raw}` has an invalid port"))?;
    Ok((host.to_string(), port))
}

/// The chunk cap and idle timeout one `connect-stream` spec asks for.
pub(super) fn limits_of(spec: &OutboundStreamSpec) -> Limits {
    Limits {
        max_frame: spec
            .max_frame
            .map_or(DEFAULT_MAX_FRAME, |frame| frame as usize),
        idle: spec.idle_timeout_seconds.map_or(DEFAULT_IDLE, |seconds| {
            Duration::from_secs(u64::from(seconds))
        }),
    }
}

/// The accept loop of one raw listener: accept, wrap the socket in TLS when the
/// declaration carries a `tls-server` record, and hand it to its own pump.
///
/// A failed handshake closes that one connection and keeps the listener serving;
/// a failed accept backs off briefly instead of spinning on a dead descriptor.
/// Each connection is handled in its own task so a slow handshake cannot stall
/// the accept loop, and it holds one of [`MAX_LISTENER_CONNECTIONS`] permits:
/// past that the socket is closed, because a peer that opens connections without
/// reading them must not grow the process without bound.
pub(super) fn spawn_accept_loop(
    listener: tokio::net::TcpListener,
    core: Arc<TransportCore>,
    app: String,
    listener_id: String,
    server_tls: Option<Arc<rustls::ServerConfig>>,
    limits: Limits,
) -> tokio::task::JoinHandle<()> {
    let protocol = if server_tls.is_some() { "tls" } else { "tcp" };
    tokio::spawn(async move {
        let permits = Arc::new(tokio::sync::Semaphore::new(MAX_LISTENER_CONNECTIONS));
        loop {
            let (socket, peer) = match listener.accept().await {
                Ok(connection) => connection,
                Err(e) => {
                    tracing::warn!(app = %app, error = %e, "app listener accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                tracing::warn!(
                    app = %app,
                    listener = %listener_id,
                    peer = %peer,
                    cap = MAX_LISTENER_CONNECTIONS,
                    "app listener connection refused: the listener is at its cap"
                );
                drop(socket);
                continue;
            };
            let mut info = StreamInfo::new(protocol);
            info.peer = peer.to_string();
            info.listener = Some(listener_id.clone());
            let stream_id = format!("stream-{}", uuid::Uuid::new_v4());
            let core = Arc::clone(&core);
            let app = app.clone();
            let listener_id = listener_id.clone();
            let server_tls = server_tls.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Some(config) = server_tls {
                    let acceptor = tokio_rustls::TlsAcceptor::from(config);
                    match acceptor.accept(socket).await {
                        Ok(stream) => {
                            let (source, sink) = byte_stream(stream, limits.max_frame);
                            core.start_stream(
                                &app,
                                &stream_id,
                                info,
                                source,
                                sink,
                                limits,
                                Some(&listener_id),
                            );
                        }
                        Err(e) => {
                            tracing::warn!(app = %app, stream = %stream_id, error = %e,
                                "app listener TLS handshake failed");
                        }
                    }
                } else {
                    let (source, sink) = byte_stream(socket, limits.max_frame);
                    core.start_stream(
                        &app,
                        &stream_id,
                        info,
                        source,
                        sink,
                        limits,
                        Some(&listener_id),
                    );
                }
            });
        }
    })
}

impl ServerStreams {
    /// `io::connect-stream` for `tls://`: a raw byte stream wrapped in the app's
    /// client TLS policy.
    pub(super) async fn connect_raw_tls(
        &self,
        app: &str,
        spec: &OutboundStreamSpec,
        dialled: Dialled,
        limits: Limits,
    ) -> Result<String> {
        // A TLS stream needs the app's material *and* the grant that allows
        // presenting it; the bridge checked the grant, this re-checks so a
        // caller that bypassed the bridge still cannot use it.
        if !self.granted(app, Capability::TlsClient) {
            return Err(BridgeError::unsupported(
                "io::connect-stream: a `tls` stream needs the `tls-client` capability",
            ));
        }
        let Dialled {
            stream: tcp,
            peer,
            host,
            port,
        } = dialled;
        let tls = spec
            .tls
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("a `tls` stream needs a `tls-client` record"))?;
        let outbound = self.client_tls(app, tls).await?;
        let config = ferris_wasm::tls::build_client_config(&outbound)?;
        let name = ferris_wasm::tls::server_name(Some(&outbound), &host)?;
        let connector = tokio_rustls::TlsConnector::from(config);
        let stream = connector
            .connect(name, tcp)
            .await
            .with_context(|| format!("TLS handshake with {host}:{port}"))?;
        let id = format!("stream-{}", uuid::Uuid::new_v4());
        let (source, sink) = byte_stream(stream, limits.max_frame);
        self.core.start_stream(
            app,
            &id,
            with_peer(StreamInfo::new("tls"), peer),
            source,
            sink,
            limits,
            None,
        );
        Ok(id)
    }
}

/// `StreamInfo` with the peer filled in.
pub(super) fn with_peer(mut info: StreamInfo, peer: String) -> StreamInfo {
    info.peer = peer;
    info
}

/// Bind every raw stream listener a declared `%stream` route carries.
///
/// Called once at boot, after the apps are loaded: a listener that cannot be
/// bound is reported and *not* retried, and the route's other surfaces stay
/// served. The alternatives — starting the server as if the listener existed, or
/// serving a declared endpoint from nowhere — are the silent success the
/// contract forbids.
///
/// A `websocket` listener is **not** a socket: it is an HTTP upgrade the router
/// mounts (see [`plugin_routes`](crate::router::plugin_routes)), so it is skipped
/// here instead of being reported as a failure.
pub async fn bind_declared_listeners(
    transport: &ServerStreams,
    declarations: &[(String, ferris_wasm::descriptor::NetworkSpec)],
) -> Vec<(String, String, String)> {
    let mut failures = Vec::new();
    for (app, network) in declarations {
        for route in &network.routes {
            if route.kind != ferris_wasm::descriptor::RouteKind::Stream {
                // An HTTP route is dispatched by the router; a listener on one
                // would be a declaration that never runs.
                continue;
            }
            let Some(listener) = &route.listener else {
                failures.push((
                    app.clone(),
                    route.id.clone(),
                    "a `%stream` route must declare a `listener` (websocket, tcp or tls): \
                     the host has no address to serve"
                        .to_string(),
                ));
                continue;
            };
            if listener.transport == ListenerTransport::Websocket {
                // An upgrade route has to be mounted by an HTTP router; the
                // router does that from the same declaration.
                tracing::debug!(
                    app = %app,
                    route = %route.id,
                    address = %listener.address.as_deref().unwrap_or("<inferred>"),
                    "declared websocket `%stream` route is mounted by the router"
                );
                continue;
            }
            match transport.listen(app, listener).await {
                Ok(id) => tracing::info!(
                    app = %app,
                    route = %route.id,
                    listener = %id,
                    address = %listener.address.as_deref().unwrap_or("<inferred>"),
                    "declared stream listener bound"
                ),
                Err(e) => {
                    tracing::error!(
                        app = %app,
                        route = %route.id,
                        address = %listener.address.as_deref().unwrap_or("<inferred>"),
                        error = %e,
                        "declared stream listener not bound"
                    );
                    failures.push((app.clone(), route.id.clone(), e.to_string()));
                }
            }
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inferred bind is loopback: a listener an app did not address must not
    /// appear on every interface.
    #[test]
    fn an_inferred_listener_binds_loopback() {
        assert!(
            bind_authority(None, None).starts_with("127.0.0.1:"),
            "the default bind host is loopback"
        );
        assert_eq!(bind_authority(None, Some(9000)), "127.0.0.1:9000");
        assert_eq!(
            bind_authority(Some("0.0.0.0"), Some(8080)),
            "0.0.0.0:8080",
            "an explicitly declared address is still honoured"
        );
        assert_eq!(bind_authority(Some("192.0.2.5:53"), None), "192.0.2.5:53");
    }
}
