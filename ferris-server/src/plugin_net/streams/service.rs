//! The `StreamService` the runtime reaches: open, listen, write, close.
//!
//! `connect` is the one place that decides which transport a spec means, so
//! `tcp`/`tls`/`ws`/`wss` cannot drift apart in their grant checks or in the
//! policy they enforce.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use ferris_wasm::host::BridgeError;
use ferris_wasm::{
    Capability, ListenerTransport, OutboundStreamSpec, StreamInfo, StreamListenerSpec,
    StreamService,
};
use futures_util::future::join_all;

use super::core::Entry;
use super::io::byte_stream;
use super::raw::{
    Dialled, connect_first, limits_of, spawn_accept_loop, split_authority, with_peer,
};
use super::{DEFAULT_IDLE, DEFAULT_MAX_FRAME, Limits, ServerStreams};
use crate::plugin_net::egress::EgressPolicy;

impl ServerStreams {
    /// The edge checks every `connect-stream` shares: the grant, the host-wide
    /// budgets and the effective outbound policy.
    ///
    /// Returns `(unrestricted, allow_private)` — the two policy bits the egress
    /// allow-list is built from.
    fn admit_connect(&self, app: &str, spec: &OutboundStreamSpec) -> Result<(bool, bool)> {
        // The bridge checks this too; re-checking here means a caller that
        // reaches the transport by another path still cannot open a socket the
        // app was not granted.
        if !self.granted(app, Capability::Streams) {
            return Err(BridgeError::unsupported(
                "io::connect-stream: opening a stream needs the `streams` capability",
            ));
        }
        // `http-unrestricted` is the operator's decision and the only thing that
        // can widen reach. The guest's `policy.allow-private` flag narrows what
        // the grant allows; on its own it used to be enough to reach loopback
        // and RFC 1918 from a sandboxed app (WASM-006), which made a guest bit
        // an operator grant.
        let unrestricted = self.granted(app, Capability::HttpUnrestricted);
        let allow_private = unrestricted
            && spec
                .policy
                .as_ref()
                .is_some_and(|policy| policy.allow_private);
        Ok((unrestricted, allow_private))
    }
}

#[async_trait]
impl StreamService for ServerStreams {
    async fn connect(&self, app: &str, spec: &OutboundStreamSpec) -> Result<String> {
        let (unrestricted, allow_private) = self.admit_connect(app, spec)?;
        if spec.transport == ListenerTransport::Websocket {
            return self
                .connect_websocket(app, spec, unrestricted, allow_private)
                .await;
        }

        let policy = EgressPolicy::from_env(unrestricted, allow_private);
        let scheme = if spec.transport == ListenerTransport::Tls {
            "tls://"
        } else {
            "tcp://"
        };
        // `address`/`port` take precedence over the authority inside `uri`, so
        // an app may pass only the scheme and name the target in the fields.
        let declared = spec
            .address
            .as_deref()
            .map(str::trim)
            .filter(|host| !host.is_empty());
        let (host, port) = match split_authority(&spec.uri, Some(scheme)) {
            Ok((uri_host, uri_port)) => (
                declared.map_or(uri_host, ToString::to_string),
                spec.port.filter(|port| *port != 0).unwrap_or(uri_port),
            ),
            Err(error) => {
                let Some(host) = declared else {
                    return Err(error);
                };
                let Some(port) = spec.port.filter(|port| *port != 0) else {
                    return Err(error)
                        .context("`port` is required when `uri` carries no authority");
                };
                (host.to_string(), port)
            }
        };
        let addresses = policy.resolve(&host, port).await?;
        let limits = limits_of(spec);
        let tcp = connect_first(&addresses)
            .await
            .with_context(|| format!("opening {scheme}{host}:{port}"))?;
        let peer = tcp
            .peer_addr()
            .map_or_else(|_| format!("{host}:{port}"), |addr| addr.to_string());
        let dialled = Dialled {
            stream: tcp,
            peer,
            host,
            port,
        };
        if spec.transport == ListenerTransport::Tls {
            return self.connect_raw_tls(app, spec, dialled, limits).await;
        }
        let Dialled { stream, peer, .. } = dialled;
        let id = format!("stream-{}", uuid::Uuid::new_v4());
        let (source, sink) = byte_stream(stream, limits.max_frame);
        self.core.start_stream(
            app,
            &id,
            with_peer(StreamInfo::new("tcp"), peer),
            source,
            sink,
            limits,
            None,
        );
        Ok(id)
    }

    async fn listen(&self, app: &str, spec: &StreamListenerSpec) -> Result<String> {
        if matches!(spec.transport, ListenerTransport::Websocket) {
            return Err(BridgeError::unimplemented(
                "io::listen-stream: a websocket listener is an HTTP upgrade and must be \
                 declared as a `%stream` route (the router mounts it), not opened at runtime",
            ));
        }
        if spec.transport.is_datagram() {
            return Err(BridgeError::unsupported(
                "io::listen-stream: datagram transports bind through `io::udp-listen` or \
                 `io::raw-open`, which carry whole datagrams instead of a byte stream",
            ));
        }
        if spec.auth.is_some() {
            // A raw socket has no request to authenticate; pretending the policy
            // was enforced would be the silent gap the contract forbids.
            return Err(BridgeError::unsupported(
                "io::listen-stream: `auth` needs an HTTP request (session/capability); a raw \
                 tcp/tls listener authenticates its own frames — declare a `%stream` route with \
                 `auth` for an upgrade the host can gate",
            ));
        }
        if !self.granted(app, Capability::StreamsListen) {
            return Err(BridgeError::unsupported(
                "io::listen-stream: binding a listener needs `streams-listen`",
            ));
        }
        self.admit_stream_listener("io::listen-stream")?;
        let bind = super::raw::bind_authority(spec.address.as_deref(), spec.port);
        let listener = tokio::net::TcpListener::bind(&bind)
            .await
            .with_context(|| format!("binding the app listener at {bind}"))?;
        let local = listener
            .local_addr()
            .map_or_else(|_| bind.clone(), |addr| addr.to_string());
        let limits = Limits {
            max_frame: spec
                .max_frame
                .map_or(DEFAULT_MAX_FRAME, |frame| frame as usize),
            idle: spec.idle_timeout_seconds.map_or(DEFAULT_IDLE, |seconds| {
                Duration::from_secs(u64::from(seconds))
            }),
        };
        let server_tls = match spec.transport {
            ListenerTransport::Tls => {
                if !self.granted(app, Capability::TlsServer) {
                    return Err(BridgeError::unsupported(
                        "io::listen-stream: a `tls` listener needs the `tls-server` capability",
                    ));
                }
                let tls = spec.tls.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("a `tls` listener needs a `tls-server` record")
                })?;
                Some(self.server_tls(app, tls).await?)
            }
            ListenerTransport::Tcp => None,
            ListenerTransport::Websocket
            | ListenerTransport::Udp
            | ListenerTransport::Raw
            | ListenerTransport::Icmp => unreachable!("handled above"),
        };

        let id = format!("listener-{}", uuid::Uuid::new_v4());
        tracing::info!(app = %app, listener = %id, address = %local, "app stream listener bound");
        let task = spawn_accept_loop(
            listener,
            Arc::clone(&self.core),
            app.to_string(),
            id.clone(),
            server_tls,
            limits,
        );
        self.core.entries.lock().insert(
            id.clone(),
            Entry {
                id: id.clone(),
                app: app.to_string(),
                sender: None,
                abort: Some(task.abort_handle()),
                done: None,
                parent: None,
            },
        );
        Ok(id)
    }

    async fn send(&self, app: &str, stream_id: &str, bytes: &[u8]) -> Result<()> {
        let sender = {
            let entries = self.core.entries.lock();
            let entry = entries
                .get(stream_id)
                .ok_or_else(|| anyhow::anyhow!("unknown or closed stream `{stream_id}`"))?;
            if entry.app != app {
                anyhow::bail!("stream `{stream_id}` does not belong to app `{app}`");
            }
            let sender = entry
                .sender
                .clone()
                .ok_or_else(|| anyhow::anyhow!("`{stream_id}` is not a stream"))?;
            drop(entries);
            sender
        };
        sender
            .send(bytes.to_vec())
            .await
            .map_err(|_| anyhow::anyhow!("stream `{stream_id}` closed before the write"))
    }

    async fn udp_connect(&self, app: &str, spec: &ferris_wasm::UdpSpec) -> Result<String> {
        Self::udp_connect(self, app, spec).await
    }

    async fn udp_listen(&self, app: &str, spec: &ferris_wasm::UdpListenerSpec) -> Result<String> {
        Self::udp_listen(self, app, spec).await
    }

    async fn udp_send(
        &self,
        app: &str,
        stream_id: &str,
        datagram: &[u8],
        peer: Option<&str>,
    ) -> Result<()> {
        Self::udp_send(self, app, stream_id, datagram, peer).await
    }

    async fn udp_close(&self, app: &str, stream_id: &str) -> Result<()> {
        Self::udp_close(self, app, stream_id).await
    }

    async fn raw_open(&self, app: &str, spec: &ferris_wasm::RawSpec) -> Result<String> {
        Self::raw_open(self, app, spec)
    }

    async fn raw_send(
        &self,
        app: &str,
        stream_id: &str,
        packet: &[u8],
        destination: Option<&str>,
    ) -> Result<()> {
        Self::raw_send(self, app, stream_id, packet, destination).await
    }

    async fn raw_close(&self, app: &str, stream_id: &str) -> Result<()> {
        Self::raw_close(self, app, stream_id).await
    }

    async fn dns_serve(&self, app: &str, zone: &ferris_wasm::DnsZone) -> Result<String> {
        Self::dns_serve(self, app, zone).await
    }

    async fn dns_update(
        &self,
        app: &str,
        listener_id: &str,
        zone: &ferris_wasm::DnsZone,
    ) -> Result<()> {
        Self::dns_update(self, app, listener_id, zone)
    }

    async fn dns_stop(&self, app: &str, listener_id: &str) -> Result<()> {
        Self::dns_stop(self, app, listener_id).await
    }

    async fn close(&self, app: &str, stream_id: &str) -> Result<()> {
        let (entry, children) = {
            let mut entries = self.core.entries.lock();
            match entries.get(stream_id) {
                Some(entry) if entry.app != app => {
                    anyhow::bail!("`{stream_id}` does not belong to app `{app}`");
                }
                Some(_) => {
                    // A listener's connections are its children: stopping the
                    // listener tears them down and reports each one, exactly as
                    // `io::close-stream` documents. `extract_if` moves them out
                    // in one pass, which is the only way to mutate the table
                    // while walking it.
                    let children: Vec<Entry> = entries
                        .extract_if(|_, entry| entry.parent.as_deref() == Some(stream_id))
                        .map(|(_, entry)| entry)
                        .collect();
                    (entries.remove(stream_id), children)
                }
                // Already closed: `io::close-stream` is idempotent on purpose, so
                // a guest's cleanup path cannot fail on a race with the peer.
                None => (None, Vec::new()),
            }
        };

        // The children close concurrently: a listener with many connections must
        // not pay the grace period once per connection.
        join_all(children.into_iter().map(|child| self.core.teardown(child))).await;

        if let Some(entry) = entry {
            self.core.teardown(entry).await;
        }
        Ok(())
    }
}
