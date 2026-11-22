//! The stream transport: the sockets behind `io::connect-stream`,
//! `io::listen-stream`, `io::send-stream` and `io::close-stream`.
//!
//! The guest never touches a socket. It asks for a connection or a listener and
//! receives a `stream-id`; this module owns the socket, announces
//! `opened`/`closed`/`error` through `handle-stream-event`, and forwards every
//! chunk to `handle-stream`, relaying the reply back to the peer. Four
//! invariants make that safe:
//!
//! * **One owner per id.** An id is registered before the guest hears about it,
//!   and `send`/`close` refuse an id that belongs to another app.
//! * **Backpressure, not buffering.** Writes from the guest go through a bounded
//!   channel: a guest that writes faster than the peer reads awaits instead of
//!   growing a buffer the host has to cap after the fact.
//! * **A byte stream is a byte stream.** `tcp`, `tls`, `ws` and `wss` differ in
//!   how the bytes travel, never in what `handle-stream` sees: one source/sink
//!   pair carries all four, so a chunk is a chunk.
//! * **Every surface is either real or an error.** A stream that cannot be
//!   opened, a listener that cannot be bound and a capability that cannot be
//!   enforced are typed failures, never an `Ok` that pretends the surface
//!   exists.
//!
//! Accepted raw connections run as an **anonymous guest**: a TCP/TLS socket has
//! no request and no session, so there is no caller to bind. `handle-stream`
//! forwards the bytes and the app authenticates its own frames (or the route
//! that declared the listener does it at accept time, which is why
//! `listen-stream` refuses an `auth` it cannot enforce).
//!
//! # WebSocket
//!
//! `ws://`/`wss://` is an outbound WebSocket **client**: the handshake is RFC
//! 6455 (text and binary frames become chunks, the guest's reply becomes one
//! binary frame), a `wss` connection is TLS-terminated by `tokio-rustls` with
//! the app's `tls-client` policy — cert, key, CA, SNI, ALPN, `verify` — so it
//! behaves exactly like an `https` call over `io::request`, and a peer frame
//! larger than `max-frame` is delivered in `max-frame` slices.
//!
//! Inbound WebSocket traffic is *not* a listener here: it is an HTTP upgrade,
//! so it belongs to the router. A `%stream` route whose listener declares
//! `transport: websocket` is mounted by
//! [`plugin_routes`](crate::router::plugin_routes), and `io::listen-stream`
//! refuses that transport with a typed error pointing at the declaration.
//!
//! # `stream-info.bytes-received`
//!
//! Every byte the host hands to `handle-stream` — from a raw socket, a
//! WebSocket frame or a declared `%stream` route — is counted by the runtime
//! (`plugin::exports::handle_stream_as`) *before* the guest sees it, so
//! `io::stream-describe` reports the running total for every transport. The
//! transport never forwards a chunk by another path, which is what keeps that
//! counter honest: there is exactly one seam (`handle_stream`) and both sides
//! use it.

mod core;
mod datagram;
mod dns;
mod io;
mod raw;
mod service;
mod tls;
mod websocket;

#[cfg(test)]
mod tests;

pub use raw::bind_declared_listeners;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ferris_wasm::{Capability, PluginStore};
use parking_lot::Mutex;

use core::TransportCore;
use datagram::DatagramTable;

/// Chunks the guest may have queued for a peer before `send-stream` awaits.
///
/// Small on purpose: the queue smooths a burst, it does not buffer an unbounded
/// download in host memory.
const WRITE_QUEUE: usize = 16;

/// Largest chunk forwarded to the guest in one call when the app sets no
/// `max-frame`.
const DEFAULT_MAX_FRAME: usize = 64 * 1024;

/// Idle timeout when the declaration sets none.
const DEFAULT_IDLE: Duration = Duration::from_secs(300);

/// How long `close-stream` lets a pump flush what the guest already queued and
/// send its close (a WebSocket Close frame, a TCP FIN, a TLS `close_notify`)
/// before the task is aborted.
///
/// Bounded on purpose: the pump may be parked inside a guest `handle-stream`
/// call, and `io::close-stream` must still return. The bound is per stream and a
/// listener's children are torn down concurrently, so closing a listener with
/// many connections does not multiply it.
const CLOSE_GRACE: Duration = Duration::from_secs(2);

/// Host-wide budget of byte-stream listeners (`tcp`/`tls`).
///
/// Before this cap an app could bind listeners until the process ran out of
/// descriptors, and a listener an operator did not ask for was indistinguishable
/// from one they did. The budget is per process, not per app, because the
/// resource is the process.
const MAX_STREAM_LISTENERS: usize = 32;

/// Host-wide budget of datagram sockets: connected UDP sockets, UDP listeners,
/// raw sockets and DNS zones (a zone holds a UDP and a TCP listener under one
/// id) share it.
const MAX_DATAGRAM_SOCKETS: usize = 32;

/// Whether the live byte-stream listener count already fills the budget.
const fn fills_listener_budget(live: usize) -> bool {
    live >= MAX_STREAM_LISTENERS
}

/// The policy one stream enforces while it runs: the chunk cap and the idle
/// timeout it was declared with.
#[derive(Debug, Clone, Copy)]
struct Limits {
    max_frame: usize,
    idle: Duration,
}

/// The process-wide stream transport.
pub struct ServerStreams {
    core: Arc<TransportCore>,
    datagrams: Arc<DatagramTable>,
}

impl std::fmt::Debug for ServerStreams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerStreams")
            .field("live", &self.live())
            .finish_non_exhaustive()
    }
}

impl ServerStreams {
    /// A transport over the loaded apps.
    #[must_use]
    pub fn new(store: Arc<PluginStore>) -> Self {
        Self {
            core: Arc::new(TransportCore {
                store,
                entries: Mutex::new(HashMap::new()),
            }),
            datagrams: Arc::new(DatagramTable::default()),
        }
    }

    /// The effective descriptor of a datagram socket or DNS listener.
    ///
    /// Byte streams answer through the guest's `stream-describe` import; this
    /// accessor is the host-side view used by tests and observability.
    #[must_use]
    pub fn describe(&self, id: &str) -> Option<ferris_wasm::StreamInfo> {
        self.datagrams.info(id)
    }

    /// How many streams and listeners are live (tests, observability).
    #[must_use]
    pub fn live(&self) -> usize {
        self.core.entries.lock().len() + self.datagrams.len()
    }

    /// Whether `app` holds an outbound capability.
    fn granted(&self, app: &str, capability: Capability) -> bool {
        self.core
            .store
            .get(app)
            .is_some_and(|plugin| plugin.grants().contains(capability))
    }

    /// How many byte-stream listeners are live: an entry with no write queue is
    /// a listener, an entry with one is an accepted connection.
    fn stream_listeners(&self) -> usize {
        self.core
            .entries
            .lock()
            .values()
            .filter(|entry| entry.sender.is_none())
            .count()
    }

    /// Refuse a new byte-stream listener past [`MAX_STREAM_LISTENERS`].
    fn admit_stream_listener(&self, call: &str) -> anyhow::Result<()> {
        if fills_listener_budget(self.stream_listeners()) {
            return Err(ferris_wasm::host::BridgeError::unsupported(format!(
                "{call}: this server already holds {MAX_STREAM_LISTENERS} byte-stream listeners"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    /// The listener budgets are host-wide constants: an app cannot raise them
    /// and the boundary is inclusive on the "full" side, which is what makes the
    /// `(N+1)`th bind the one that fails.
    #[test]
    fn the_listener_budget_refuses_the_socket_past_the_limit() {
        assert!(!fills_listener_budget(MAX_STREAM_LISTENERS - 1));
        assert!(fills_listener_budget(MAX_STREAM_LISTENERS));
        assert!(fills_listener_budget(MAX_STREAM_LISTENERS + 1));
    }
}
