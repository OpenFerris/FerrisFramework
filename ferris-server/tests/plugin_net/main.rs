//! The socket half: app streams, the WebSocket client and the app's
//! own TLS listener.
//!
//! Every test here drives real loopback sockets — no external network, no mocks
//! of the transport — because the contract being verified is precisely that the
//! host owns the socket: `connect-stream` reaches a peer (raw or WebSocket),
//! `send-stream` writes to it, a declared `%stream` route upgrades at its own
//! path behind its own auth, and a `tls-server` route is served with the app's
//! certificate on its own listener.
//!
//! The guest is `probe-ok` (the shared fixture). Build it first; the tests skip
//! loudly when it is absent:
//! ```sh
//! cargo build --target wasm32-wasip2 \
//!   --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml
//! ```

#![allow(unused_crate_dependencies)]

mod datagram;
mod declared;
mod process;
mod streams;
mod support;
mod tls;
