//! The `dns-*` listener lifecycle and the UDP/TCP pumps.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use ferris_wasm::host::BridgeError;
use ferris_wasm::{Capability, DnsZone, StreamInfo};
use parking_lot::RwLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::task::AbortHandle;

use super::super::ServerStreams;
use super::super::datagram::{DatagramEntry, DatagramKind};
use super::{DEFAULT_DNS_HOST, DnsListener, MAX_DNS_TCP_CONNECTIONS, MAX_QUERY, Zone};

impl ServerStreams {
    /// The common checks for every `dns-*` call.
    fn admit_dns(&self, app: &str, call: &str) -> Result<()> {
        if !self.granted(app, Capability::DnsServer) {
            return Err(BridgeError::unsupported(format!(
                "{call}: serving DNS needs the `dns-server` capability"
            )));
        }
        Ok(())
    }

    fn zone_of(&self, app: &str, listener_id: &str) -> Result<Option<Arc<RwLock<Zone>>>> {
        self.datagrams().zone(app, listener_id)
    }
}

impl ServerStreams {
    pub(in crate::plugin_net::streams) async fn dns_serve(
        &self,
        app: &str,
        zone: &DnsZone,
    ) -> Result<String> {
        self.admit_dns(app, "io::dns-serve")?;
        self.datagrams()
            .admit("io::dns-serve", super::super::MAX_DATAGRAM_SOCKETS)?;
        let parsed = Zone::parse(zone)?;
        let host = dns_host(zone.address.as_deref());
        let ip: IpAddr = host
            .parse()
            .with_context(|| format!("io::dns-serve: `{host}` is not an IP address"))?;
        let requested = zone.port;
        let (udp, tcp, port) = bind_pair(ip, requested).await?;
        let local = format!("{ip}:{port}");
        let zone = Arc::new(RwLock::new(parsed));
        let id = format!("dns-{}", uuid::Uuid::new_v4());
        let udp_task = spawn_dns_udp(udp, Arc::clone(&zone));
        let tcp_task = spawn_dns_tcp(tcp, Arc::clone(&zone));
        let info = StreamInfo::new("udp")
            .protocol("dns")
            .endpoints(Some(local), None, Some(port));
        self.register_datagram(
            app,
            &id,
            info.clone(),
            DatagramEntry {
                app: app.to_string(),
                kind: DatagramKind::Dns(DnsListener { zone }),
                tasks: vec![udp_task, tcp_task],
                info,
            },
        );
        Ok(id)
    }

    pub(in crate::plugin_net::streams) fn dns_update(
        &self,
        app: &str,
        listener_id: &str,
        zone: &DnsZone,
    ) -> Result<()> {
        self.admit_dns(app, "io::dns-update")?;
        let Some(shared) = self.zone_of(app, listener_id)? else {
            anyhow::bail!("io::dns-update: unknown or closed DNS listener `{listener_id}`");
        };
        let parsed = Zone::parse(zone)?;
        {
            let mut current = shared.write();
            if !current.same_origin(&parsed) {
                anyhow::bail!(
                    "io::dns-update: the listener serves `{}`; refusing a zone for `{}`",
                    current.origin,
                    parsed.origin
                );
            }
            *current = parsed;
        }
        Ok(())
    }

    // See `udp_close`: the forwarded future is real, but clippy loses the
    // await through the shared `close_datagram` helper.
    #[allow(clippy::unused_async)]
    pub(in crate::plugin_net::streams) async fn dns_stop(
        &self,
        app: &str,
        listener_id: &str,
    ) -> Result<()> {
        self.close_datagram(app, listener_id).await
    }
}

/// The address a zone binds: the declared one, or loopback.
pub(super) fn dns_host(address: Option<&str>) -> &str {
    address
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .unwrap_or(DEFAULT_DNS_HOST)
}

/// Bind UDP and TCP on the same numeric port.
///
/// Port `0` always asks for an inferred one; `None` asks for 53 and falls back
/// to an inferred port when the process may not bind it, which is the honest
/// behaviour for an unprivileged deployment (the guest reads the effective port
/// from `stream-describe`).
async fn bind_pair(ip: IpAddr, requested: Option<u16>) -> Result<(UdpSocket, TcpListener, u16)> {
    let preferred = requested.unwrap_or(53);
    let udp = match requested {
        Some(port) => UdpSocket::bind(SocketAddr::new(ip, port))
            .await
            .with_context(|| format!("io::dns-serve: binding UDP {ip}:{port}"))?,
        None => match UdpSocket::bind(SocketAddr::new(ip, preferred)).await {
            Ok(socket) => socket,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                UdpSocket::bind(SocketAddr::new(ip, 0))
                    .await
                    .with_context(|| format!("io::dns-serve: binding UDP {ip}:0"))?
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("io::dns-serve: binding UDP {ip}:{preferred}"));
            }
        },
    };
    let port = udp
        .local_addr()
        .with_context(|| format!("io::dns-serve: reading the UDP address {ip}"))?
        .port();
    let tcp = TcpListener::bind(SocketAddr::new(ip, port))
        .await
        .with_context(|| format!("io::dns-serve: binding TCP {ip}:{port}"))?;
    Ok((udp, tcp, port))
}

fn spawn_dns_udp(socket: UdpSocket, zone: Arc<RwLock<Zone>>) -> AbortHandle {
    let task = tokio::spawn(async move {
        let mut buffer = vec![0_u8; MAX_QUERY];
        loop {
            let Ok((size, peer)) = socket.recv_from(&mut buffer).await else {
                break;
            };
            let query = &buffer[..size];
            let response = zone.read().answer(query, true);
            if let Some(response) = response
                && let Err(error) = socket.send_to(&response, peer).await
            {
                tracing::debug!(peer = %peer, error = %error, "DNS UDP reply failed");
            }
        }
    });
    task.abort_handle()
}

fn spawn_dns_tcp(listener: TcpListener, zone: Arc<RwLock<Zone>>) -> AbortHandle {
    let permits = Arc::new(tokio::sync::Semaphore::new(MAX_DNS_TCP_CONNECTIONS));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                break;
            };
            // The connection is refused, not queued: a peer that opens sockets
            // without reading them must not grow the host's task set without
            // bound. It is logged because a truncated zone transfer is exactly
            // what an operator has to be able to explain.
            let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                tracing::warn!(
                    peer = %peer,
                    cap = MAX_DNS_TCP_CONNECTIONS,
                    "DNS TCP connection refused: the zone is at its connection cap"
                );
                drop(stream);
                continue;
            };
            let zone = Arc::clone(&zone);
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(error) = serve_tcp(stream, zone).await {
                    tracing::debug!(peer = %peer, error = %error, "DNS TCP query failed");
                }
            });
        }
    });
    task.abort_handle()
}

async fn serve_tcp(mut stream: TcpStream, zone: Arc<RwLock<Zone>>) -> Result<()> {
    let mut length = [0_u8; 2];
    stream.read_exact(&mut length).await?;
    let size = usize::from(u16::from_be_bytes(length));
    if size == 0 || size > MAX_QUERY {
        anyhow::bail!("a DNS TCP query of {size} bytes is out of range");
    }
    let mut query = vec![0_u8; size];
    stream.read_exact(&mut query).await?;
    let response = zone
        .read()
        .answer(&query, false)
        .ok_or_else(|| anyhow::anyhow!("the packet is not a DNS query"))?;
    let length = u16::try_from(response.len()).unwrap_or(u16::MAX);
    stream.write_all(&length.to_be_bytes()).await?;
    stream.write_all(&response).await?;
    stream.flush().await?;
    Ok(())
}
