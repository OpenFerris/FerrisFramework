//! `udp-connect`, `udp-listen`, `udp-send` and `udp-close`.

use std::sync::Arc;

use anyhow::{Context, Result};
use ferris_wasm::host::BridgeError;
use ferris_wasm::{UdpListenerSpec, UdpSpec};
use tokio::net::UdpSocket;

use super::super::raw::bind_authority;
use super::parse::resolve_peer;
use super::pump::spawn_udp_pump;
use super::{
    DEFAULT_DATAGRAM_HOST, DatagramEntry, DatagramKind, MAX_UDP_DATAGRAM, ServerStreams,
    datagram_info,
};

impl ServerStreams {
    pub(in crate::plugin_net::streams) async fn udp_connect(
        &self,
        app: &str,
        spec: &UdpSpec,
    ) -> Result<String> {
        self.admit_udp(app, "io::udp-connect")?;
        let Some(port) = spec.port.filter(|port| *port != 0) else {
            anyhow::bail!("io::udp-connect: `{}` needs a peer port", spec.address);
        };
        // The peer goes through the same allow-list and private-address rule as
        // a `connect-stream` target: a UDP socket to a host the app could not
        // call over TCP was the same hole with a different protocol on it.
        let policy = self.datagram_policy(app);
        let remote = policy
            .resolve(spec.address.trim(), port)
            .await
            .and_then(|addresses| {
                addresses.into_iter().next().ok_or_else(|| {
                    anyhow::anyhow!(
                        "io::udp-connect: `{}:{port}` resolved to no address",
                        spec.address.trim()
                    )
                })
            })
            .map_err(|error| anyhow::anyhow!("io::udp-connect: {error}"))?;
        // The local half of an outbound socket is not a listener: binding the
        // wildcard address only picks the source interface for a socket already
        // pinned to `remote`, so it is not the exposure `admit_bind` refuses.
        let local_host = spec
            .local_address
            .as_deref()
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .map_or_else(|| "0.0.0.0".to_string(), ToString::to_string);
        let local_port = spec.local_port.filter(|port| *port != 0).unwrap_or(0);
        let bind = bind_authority(Some(&local_host), Some(local_port));
        let socket = UdpSocket::bind(&bind)
            .await
            .with_context(|| format!("io::udp-connect: binding {bind}"))?;
        if spec.broadcast {
            socket
                .set_broadcast(true)
                .context("io::udp-connect: enabling broadcast")?;
        }
        if let Some(ttl) = spec.ttl {
            socket
                .set_ttl(u32::from(ttl))
                .context("io::udp-connect: setting the TTL")?;
        }
        socket
            .connect(remote)
            .await
            .with_context(|| format!("io::udp-connect: connecting to {remote}"))?;
        let local = socket
            .local_addr()
            .map_or_else(|_| bind.clone(), |addr| addr.to_string());
        let port = socket.local_addr().map_or(local_port, |addr| addr.port());
        let id = format!("udp-{}", uuid::Uuid::new_v4());
        let socket = Arc::new(socket);
        let task = spawn_udp_pump(
            Arc::clone(&self.core.store),
            app.to_string(),
            id.clone(),
            Arc::clone(&socket),
            true,
            MAX_UDP_DATAGRAM,
        );
        let info = datagram_info("udp", local, Some(remote.to_string()), port);
        self.register_datagram(
            app,
            &id,
            info.clone(),
            DatagramEntry {
                app: app.to_string(),
                kind: DatagramKind::Udp {
                    socket,
                    connected: true,
                    max: MAX_UDP_DATAGRAM,
                },
                tasks: vec![task],
                info,
            },
        );
        Ok(id)
    }

    pub(in crate::plugin_net::streams) async fn udp_listen(
        &self,
        app: &str,
        spec: &UdpListenerSpec,
    ) -> Result<String> {
        self.admit_udp(app, "io::udp-listen")?;
        self.datagrams()
            .admit("io::udp-listen", super::super::MAX_DATAGRAM_SOCKETS)?;
        let host = spec
            .address
            .as_deref()
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .unwrap_or(DEFAULT_DATAGRAM_HOST);
        self.admit_bind(app, host, "io::udp-listen")?;
        let port = spec.port.filter(|port| *port != 0).unwrap_or(0);
        let bind = bind_authority(Some(host), Some(port));
        let socket = UdpSocket::bind(&bind)
            .await
            .with_context(|| format!("io::udp-listen: binding {bind}"))?;
        if spec.broadcast {
            socket
                .set_broadcast(true)
                .context("io::udp-listen: enabling broadcast")?;
        }
        let local_addr = socket
            .local_addr()
            .with_context(|| format!("io::udp-listen: reading the address of {bind}"))?;
        let max = spec
            .max_datagram
            .map_or(MAX_UDP_DATAGRAM, |value| value as usize)
            .clamp(1, MAX_UDP_DATAGRAM);
        let id = format!("udp-{}", uuid::Uuid::new_v4());
        let socket = Arc::new(socket);
        let task = spawn_udp_pump(
            Arc::clone(&self.core.store),
            app.to_string(),
            id.clone(),
            Arc::clone(&socket),
            false,
            max,
        );
        self.register_datagram(
            app,
            &id,
            datagram_info("udp", local_addr.to_string(), None, local_addr.port()),
            DatagramEntry {
                app: app.to_string(),
                kind: DatagramKind::Udp {
                    socket,
                    connected: false,
                    max,
                },
                tasks: vec![task],
                info: datagram_info("udp", local_addr.to_string(), None, local_addr.port()),
            },
        );
        Ok(id)
    }

    pub(in crate::plugin_net::streams) async fn udp_send(
        &self,
        app: &str,
        stream_id: &str,
        datagram: &[u8],
        peer: Option<&str>,
    ) -> Result<()> {
        self.admit_udp(app, "io::udp-send")?;
        let peer = match peer {
            Some(raw) => {
                let address = resolve_peer(raw).await?;
                // A listener's pump answers the source of a datagram it actually
                // received; a guest naming a peer is choosing where to send, so
                // the private rule applies (WASM-005). The allow-list gates the
                // `udp-connect` target, which is where the socket is created.
                if !self.datagram_policy(app).allows_address(address.ip()) {
                    return Err(BridgeError::unsupported(format!(
                        "io::udp-send: peer `{raw}` is a private address and this app does not \
                         hold `http-unrestricted`"
                    )));
                }
                Some(address)
            }
            None => None,
        };
        self.datagram_send(app, stream_id, datagram, peer, "io::udp-send")
            .await
    }

    // The body awaits the shared close path; clippy's `unused_async` misfires
    // on the `let _ = run_guest(...).await` shape inside `close_datagram`.
    #[allow(clippy::unused_async)]
    pub(in crate::plugin_net::streams) async fn udp_close(
        &self,
        app: &str,
        stream_id: &str,
    ) -> Result<()> {
        self.close_datagram(app, stream_id).await
    }
}
