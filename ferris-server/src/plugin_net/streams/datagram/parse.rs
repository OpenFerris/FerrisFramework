//! Peer/destination parsing shared by the datagram calls.

use std::net::{IpAddr, SocketAddr};

use anyhow::{Context, Result};

/// Parse a `"address:port"` peer, resolving a host name when one is given.
pub(super) async fn resolve_peer(raw: &str) -> Result<SocketAddr> {
    let raw = raw.trim();
    if raw.is_empty() {
        anyhow::bail!("an empty peer address");
    }
    // A bare address has no port and is not a valid UDP destination; saying so
    // is better than silently picking one.
    if raw.parse::<IpAddr>().is_ok() {
        anyhow::bail!("peer `{raw}` needs a port (`address:port`)");
    }
    let mut resolved = tokio::net::lookup_host(raw)
        .await
        .with_context(|| format!("resolving the datagram peer `{raw}`"))?;
    resolved
        .next()
        .ok_or_else(|| anyhow::anyhow!("peer `{raw}` resolved to no address"))
}

/// Parse a bare IP for a raw destination, with an optional `:port` the packet
/// header owns (the host never rewrites it).
pub(super) fn raw_destination(raw: &str) -> Result<IpAddr> {
    let raw = raw.trim();
    if let Ok(ip) = raw.parse::<IpAddr>() {
        return Ok(ip);
    }
    if let Ok(peer) = raw.parse::<SocketAddr>() {
        return Ok(peer.ip());
    }
    anyhow::bail!("raw destination `{raw}` is not an IP address")
}
