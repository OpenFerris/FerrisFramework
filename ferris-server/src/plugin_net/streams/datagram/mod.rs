//! UDP and raw sockets: the datagram half of `io`.
//!
//! §L4 of the 0.3 contract added UDP, authoritative DNS and raw/ICMP sockets.
//! They share the same shape as the byte streams: the guest asks for a socket,
//! the host opens it, and every inbound datagram reaches the guest through
//! `handle-datagram` (not `handle-stream`, which carries a byte stream). The
//! registry below is deliberately separate from [`super::core::Entry`]: a
//! datagram has no writer queue or carry-over buffer, so forcing it into the
//! byte-stream table would mean three `Option`s that are always `None`.
//!
//! The per-call surface lives in [`udp`] and [`raw`], their shared admission
//! policy in [`policy`], and the receive loop in [`pump`].
//!
//! # Failure isolation
//!
//! The socket is owned by the host, not the guest. A guest that traps or spins
//! stops receiving, but the descriptor is closed by the abort handle as soon as
//! the entry is torn down; the OS never keeps a leaked socket alive after the
//! app is gone. A datagram larger than the cap is refused at the edge before it
//! reaches the guest, so one app cannot exhaust host memory with a jumbo frame.
//!
//! # Privileges
//!
//! UDP needs none. Raw and ICMP sockets do, and the operating system answers
//! `EPERM`; that is reported as a typed `permission-denied` bridge error with
//! the action the operator must take, never as a socket that pretends to be
//! open.

mod parse;
mod policy;
mod pump;
mod raw;
mod udp;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use ferris_wasm::host::BridgeError;
use ferris_wasm::{StreamEvent, StreamInfo};
use parking_lot::Mutex;
use tokio::net::UdpSocket;
use tokio::task::AbortHandle;

use super::ServerStreams;
use super::dns::DnsListener;
use crate::plugin_net::run_guest;

/// Largest UDP payload the host accepts or forwards in one call.
///
/// 65 507 is the practical maximum of a UDP datagram over IPv4; keeping the
/// receive buffer at that size means a legal datagram is never truncated. The
/// guest's own `max-datagram` is applied on top and clamped here.
const MAX_UDP_DATAGRAM: usize = 65_507;

/// Largest raw IP packet forwarded in one `handle-datagram` call.
///
/// The IPv4 total-length field is 16 bits, so this is the protocol ceiling. A
/// raw socket that carries IPv6 can be larger in theory, but no network path
/// the host serves does, and a bounded buffer is what keeps a malicious peer
/// from making the host allocate per packet.
const MAX_RAW_PACKET: usize = 65_535;

/// The bind address a UDP listener or raw socket uses when the app declares
/// none: loopback, never every interface (WASM-013/014).
const DEFAULT_DATAGRAM_HOST: &str = "127.0.0.1";

/// One live datagram socket, owned by the server and attributed to an app.
pub(super) enum DatagramKind {
    Udp {
        socket: Arc<UdpSocket>,
        /// A connected socket has a fixed peer; a listener accepts any sender.
        connected: bool,
        /// Largest datagram forwarded to the guest.
        max: usize,
    },
    Raw {
        socket: Arc<UdpSocket>,
        max: usize,
    },
    Dns(DnsListener),
}

/// The table entry: ownership, effective endpoints and the tasks that pump it.
pub(super) struct DatagramEntry {
    pub(super) app: String,
    pub(super) kind: DatagramKind,
    pub(super) tasks: Vec<AbortHandle>,
    /// The effective descriptor handed to the guest (addresses and port).
    pub(super) info: StreamInfo,
}

/// Every UDP, raw and DNS descriptor this process holds.
#[derive(Default)]
pub(super) struct DatagramTable {
    entries: Mutex<HashMap<String, DatagramEntry>>,
}

impl DatagramTable {
    pub(super) fn insert(&self, id: String, entry: DatagramEntry) {
        self.entries.lock().insert(id, entry);
    }

    pub(super) fn info(&self, id: &str) -> Option<StreamInfo> {
        self.entries.lock().get(id).map(|entry| entry.info.clone())
    }

    pub(super) fn len(&self) -> usize {
        self.entries.lock().len()
    }

    /// Refuse a new socket past the host-wide budget.
    ///
    /// The budget is shared by UDP, raw and DNS descriptors because they all
    /// cost the process a descriptor and a pump task; before it, one app could
    /// open them until the host ran out.
    pub(super) fn admit(&self, call: &str, max: usize) -> Result<()> {
        if self.len() >= max {
            return Err(BridgeError::unsupported(format!(
                "{call}: this server already holds {max} datagram sockets (UDP, raw and DNS \
                 share the budget)"
            )));
        }
        Ok(())
    }

    /// The live zone of a DNS listener this app owns.
    pub(super) fn zone(
        &self,
        app: &str,
        id: &str,
    ) -> anyhow::Result<Option<Arc<parking_lot::RwLock<super::dns::Zone>>>> {
        let entries = self.entries.lock();
        let Some(entry) = entries.get(id) else {
            drop(entries);
            return Ok(None);
        };
        let owned = if entry.app == app {
            match &entry.kind {
                DatagramKind::Dns(listener) => Ok(Arc::clone(&listener.zone)),
                _ => Err(anyhow::anyhow!("`{id}` is not a DNS listener")),
            }
        } else {
            Err(anyhow::anyhow!("`{id}` does not belong to app `{app}`"))
        };
        drop(entries);
        Ok(Some(owned?))
    }

    /// Remove `id` after checking it belongs to `app`.
    ///
    /// Returns `Ok(None)` when the id is unknown (the close functions are
    /// idempotent by contract) and an error when another app owns it.
    fn take(&self, app: &str, id: &str) -> Result<Option<DatagramEntry>> {
        let mut entries = self.entries.lock();
        let taken = match entries.get(id) {
            Some(entry) if entry.app != app => {
                anyhow::bail!("`{id}` does not belong to app `{app}`")
            }
            Some(_) => entries.remove(id),
            None => None,
        };
        drop(entries);
        Ok(taken)
    }
}

impl ServerStreams {
    pub(super) const fn datagrams(&self) -> &Arc<DatagramTable> {
        &self.datagrams
    }

    /// Announce a datagram socket to the guest and register its `stream-info`.
    pub(super) fn register_datagram(
        &self,
        app: &str,
        id: &str,
        info: StreamInfo,
        entry: DatagramEntry,
    ) {
        self.datagrams().insert(id.to_string(), entry);
        if let Err(e) = self.core.store.register_stream(app, id, info) {
            // The socket is real even if the guest's own registry rejected it:
            // tearing it down would turn a name collision into a lost socket.
            tracing::warn!(app = %app, stream = %id, error = %e, "datagram registration failed");
        }
    }

    /// Report `closed` once and forget the descriptor.
    ///
    /// `udp-close`/`raw-close`/`dns-stop` are idempotent; only the first call
    /// that actually removes an entry emits the terminal event, so a guest that
    /// races cleanup with a peer cannot receive two `closed` events.
    pub(super) async fn close_datagram(&self, app: &str, id: &str) -> Result<()> {
        let Some(entry) = self.datagrams().take(app, id)? else {
            return Ok(());
        };
        for task in &entry.tasks {
            task.abort();
        }
        let _ = self.core.store.unregister_stream(app, id);
        let store = Arc::clone(&self.core.store);
        let app = app.to_string();
        let id = id.to_string();
        let _ = run_guest(move || {
            store
                .handle_stream_event(&app, &id, StreamEvent::Closed)
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
        .await;
        Ok(())
    }

    /// The socket behind `id`, after ownership and kind checks.
    fn datagram_socket(
        &self,
        app: &str,
        id: &str,
        what: &str,
    ) -> Result<(Arc<UdpSocket>, bool, usize)> {
        let entries = self.datagrams().entries.lock();
        let Some(entry) = entries.get(id) else {
            drop(entries);
            return Err(anyhow::anyhow!("unknown or closed socket `{id}`"));
        };
        let owned = if entry.app == app {
            match &entry.kind {
                DatagramKind::Udp {
                    socket,
                    connected,
                    max,
                } => Ok((Arc::clone(socket), *connected, *max)),
                DatagramKind::Raw { socket, max, .. } => Ok((Arc::clone(socket), false, *max)),
                DatagramKind::Dns(_) => Err(BridgeError::unsupported(format!(
                    "{what}: `{id}` is a DNS listener, not a datagram socket"
                ))),
            }
        } else {
            Err(anyhow::anyhow!("`{id}` does not belong to app `{app}`"))
        };
        drop(entries);
        owned
    }

    /// Send one payload from a live socket, refusing another app's id.
    async fn datagram_send(
        &self,
        app: &str,
        id: &str,
        payload: &[u8],
        peer: Option<SocketAddr>,
        what: &str,
    ) -> Result<()> {
        let (socket, connected, max) = self.datagram_socket(app, id, what)?;
        if payload.len() > max {
            return Err(BridgeError::unsupported(format!(
                "{what}: {} bytes exceeds the {max} byte datagram cap",
                payload.len()
            )));
        }
        match (connected, peer) {
            (true, None) => socket
                .send(payload)
                .await
                .map(|_| ())
                .with_context(|| format!("{what}: write to the connected peer failed")),
            (_, Some(peer)) => socket
                .send_to(payload, peer)
                .await
                .map(|_| ())
                .with_context(|| format!("{what}: write to {peer} failed")),
            (false, None) => Err(BridgeError::unsupported(format!(
                "{what}: a listener needs `peer` (`\"address:port\"`)"
            ))),
        }
    }
}

fn datagram_info(protocol: &str, local: String, remote: Option<String>, port: u16) -> StreamInfo {
    StreamInfo::new(protocol)
        .protocol(protocol)
        .endpoints(Some(local), remote, Some(port))
}
