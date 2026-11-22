//! Authoritative DNS server: the host side of `io::dns-serve`.
//!
//! The contract is explicit that the **host** parses and answers DNS: the app
//! supplies a zone (records and an optional typed SOA), never a raw packet
//! encoder. That keeps the wire format in one audited place and lets a guest
//! written in any language publish a zone without implementing DNS.
//!
//! `dns-serve` binds UDP and TCP on the same port (53 by default, an inferred
//! one when the app asks for `0` or when the host may not bind 53), which is
//! what an authoritative server has to do: UDP for ordinary queries and TCP for
//! truncated answers and zone transfers. `dns-update` swaps the zone behind a
//! lock, so a query in flight sees either the old zone or the new one, never a
//! half-applied mix.
//!
//! [`zone`] holds the parsed zone and the wire codec; [`service`] the listener
//! lifecycle and its pumps.

mod service;
mod zone;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use ferris_wasm::DnsSoa;
use parking_lot::RwLock;

/// Largest DNS message the host reads from one query.
const MAX_QUERY: usize = 4096;

/// Largest UDP answer before the TC bit asks the client to retry over TCP.
const MAX_UDP_ANSWER: usize = 1232;

/// Records one zone may carry.
///
/// A zone is parsed once and walked per query, so its size is the per-query
/// cost; without a cap an app could publish a zone whose answer loop is the
/// denial of service. 512 names is far past a real app's zone.
const MAX_DNS_RECORDS: usize = 512;

/// TCP queries one zone serves concurrently.
///
/// Each accepted connection is a task and an answer buffer; the cap keeps a
/// peer that opens sockets without reading them from exhausting the process.
const MAX_DNS_TCP_CONNECTIONS: usize = 64;

/// The bind address a zone uses when the declaration names none: loopback, not
/// every interface (WASM-013/014).
const DEFAULT_DNS_HOST: &str = "127.0.0.1";

/// Default record TTL when the declaration says `0`.
const DEFAULT_TTL: u32 = 300;

/// Default SOA timing when the typed record leaves a field at `0`.
const DEFAULT_SOA_REFRESH: u32 = 3600;
const DEFAULT_SOA_RETRY: u32 = 600;
const DEFAULT_SOA_EXPIRE: u32 = 86_400;
const DEFAULT_SOA_MINIMUM: u32 = 300;

const TYPE_A: u16 = 1;
const TYPE_NS: u16 = 2;
const TYPE_CNAME: u16 = 5;
const TYPE_SOA: u16 = 6;
const TYPE_PTR: u16 = 12;
const TYPE_MX: u16 = 15;
const TYPE_TXT: u16 = 16;
const TYPE_AAAA: u16 = 28;
const TYPE_SRV: u16 = 33;
const TYPE_ANY: u16 = 255;
const CLASS_IN: u16 = 1;
const RCODE_NOERROR: u16 = 0;
const RCODE_FORMERR: u16 = 1;
const RCODE_NXDOMAIN: u16 = 3;
const RCODE_REFUSED: u16 = 5;

/// One resource record, already validated and encoded.
struct Record {
    owner: String,
    rtype: u16,
    ttl: u32,
    priority: Option<u16>,
    data: String,
}

/// A parsed, validated zone. `serial` is derived once and reused until the
/// records change, so serving the same zone twice does not bump it.
pub(super) struct Zone {
    origin: String,
    records: Vec<Record>,
    soa: Option<DnsSoa>,
}

/// The live state of a `dns-serve` listener.
pub(super) struct DnsListener {
    pub(super) zone: Arc<RwLock<Zone>>,
}
