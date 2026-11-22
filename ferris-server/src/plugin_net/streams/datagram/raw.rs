//! `raw-open`, `raw-send` and `raw-close`.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use ferris_wasm::host::BridgeError;
use ferris_wasm::{RawSpec, RawTransport};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;

use super::super::raw::bind_authority;
use super::parse::raw_destination;
use super::pump::spawn_udp_pump;
use super::{
    DEFAULT_DATAGRAM_HOST, DatagramEntry, DatagramKind, MAX_RAW_PACKET, ServerStreams,
    datagram_info,
};

impl ServerStreams {
    pub(in crate::plugin_net::streams) fn raw_open(
        &self,
        app: &str,
        spec: &RawSpec,
    ) -> Result<String> {
        self.admit_raw(app, "io::raw-open")?;
        self.datagrams()
            .admit("io::raw-open", super::super::MAX_DATAGRAM_SOCKETS)?;
        let protocol = match (spec.transport, spec.ip_protocol) {
            (RawTransport::Icmp, _) => 1,
            (RawTransport::Raw, Some(number)) => number,
            (RawTransport::Raw, None) => 0,
        };
        if matches!(spec.transport, RawTransport::Icmp) && spec.ip_protocol.is_some_and(|p| p != 1)
        {
            return Err(BridgeError::unsupported(
                "io::raw-open: an `icmp` socket has IP protocol 1; declare `raw` for another protocol",
            ));
        }
        let host = spec
            .address
            .as_deref()
            .map(str::trim)
            .filter(|host| !host.is_empty())
            .unwrap_or(DEFAULT_DATAGRAM_HOST);
        self.admit_bind(app, host, "io::raw-open")?;
        let bind = bind_authority(Some(host), spec.port);
        let address: SocketAddr = bind
            .parse()
            .with_context(|| format!("io::raw-open: `{bind}` is not an IP address"))?;
        let domain = if address.is_ipv6() {
            Domain::IPV6
        } else {
            Domain::IPV4
        };
        let socket = Socket::new(domain, Type::RAW, Some(Protocol::from(i32::from(protocol))))
            .with_context(|| {
                format!(
                    "io::raw-open: creating a raw socket needs CAP_NET_RAW (or root); \
                 the operating system refused SOCK_RAW for protocol {protocol}"
                )
            })?;
        socket
            .bind(&address.into())
            .with_context(|| format!("io::raw-open: binding {bind}"))?;
        socket
            .set_nonblocking(true)
            .context("io::raw-open: making the socket non-blocking")?;
        let std_socket: std::net::UdpSocket = socket.into();
        let socket =
            UdpSocket::from_std(std_socket).context("io::raw-open: registering the socket")?;
        let local = socket
            .local_addr()
            .map_or_else(|_| host.to_string(), |addr| addr.ip().to_string());
        let port = socket.local_addr().map_or(0, |addr| addr.port());
        let transport = match spec.transport {
            RawTransport::Raw => "raw",
            RawTransport::Icmp => "icmp",
        };
        let id = format!("raw-{}", uuid::Uuid::new_v4());
        let socket = Arc::new(socket);
        let task = spawn_udp_pump(
            Arc::clone(&self.core.store),
            app.to_string(),
            id.clone(),
            Arc::clone(&socket),
            false,
            MAX_RAW_PACKET,
        );
        let info = datagram_info(transport, local, None, port);
        self.register_datagram(
            app,
            &id,
            info.clone(),
            DatagramEntry {
                app: app.to_string(),
                kind: DatagramKind::Raw {
                    socket,
                    max: MAX_RAW_PACKET,
                },
                tasks: vec![task],
                info,
            },
        );
        Ok(id)
    }

    pub(in crate::plugin_net::streams) async fn raw_send(
        &self,
        app: &str,
        stream_id: &str,
        packet: &[u8],
        destination: Option<&str>,
    ) -> Result<()> {
        self.admit_raw(app, "io::raw-send")?;
        let peer = match destination {
            Some(raw) => {
                let ip = raw_destination(raw)?;
                // A raw destination is always a literal address, so the
                // allow-list has nothing to match; the private rule is what
                // keeps a sandboxed app from aiming host-built packets at
                // loopback or RFC 1918 (WASM-005).
                if !self.datagram_policy(app).allows_address(ip) {
                    return Err(BridgeError::unsupported(format!(
                        "io::raw-send: destination `{raw}` is a private address and this app does \
                         not hold `http-unrestricted`"
                    )));
                }
                Some(SocketAddr::new(ip, 0))
            }
            None => None,
        };
        self.datagram_send(app, stream_id, packet, peer, "io::raw-send")
            .await
    }

    pub(in crate::plugin_net::streams) async fn raw_close(
        &self,
        app: &str,
        stream_id: &str,
    ) -> Result<()> {
        self.close_datagram(app, stream_id).await
    }
}
