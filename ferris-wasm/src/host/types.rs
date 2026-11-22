//! Wire records the host and the WIT imports exchange.
//!
//! These are the host's own types on purpose: [`HostApi`](super::HostApi) stays
//! free of the generated WIT records, and the bridge converts at the boundary.

use serde::{Deserialize, Serialize};

/// A document as the framework hands it to a plugin. `data` is the JSON body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocRecord {
    pub name: String,
    pub doctype: String,
    pub owner: String,
    pub docstatus: String,
    pub data: String,
}

/// Transport metadata of a live stream the host forwards into
/// `handle-stream` (transport, remote peer, age and byte count), or of a
/// datagram socket the host forwards into `handle-datagram`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamInfo {
    /// The wire `stream-info.transport`: "websocket" | "tcp" | "tls" | "udp" |
    /// "raw" | "icmp" | "rest" (the HTTP-framed dev transport).
    pub transport: String,
    /// Best-effort "ip:port", empty when unknown.
    pub peer: String,
    /// Whether the host rejects anonymous frames for this stream at the edge.
    pub auth_required: bool,
    /// Unix epoch milliseconds when the stream was accepted.
    pub created_at: u64,
    /// Total bytes forwarded into `handle-stream` (or `handle-datagram`) for
    /// this stream so far.
    pub bytes_received: u64,
    /// Client certificate identity of the connection that opened the stream,
    /// when the host terminated mutual TLS for it. `None` for plain HTTP or an
    /// anonymous client; carried straight through to `streams::info`.
    pub client_identity: Option<ferris_core::ClientIdentity>,
    /// Opaque token that changes every time this `stream-id` is re-registered
    /// by a **different connection** (a changed `client_identity`). Unlike
    /// `created_at`, which `register_stream` preserves across a REST
    /// re-registration of the same stream to keep the carry-over buffer alive,
    /// this is the explicit recreation signal a guest compares to invalidate
    /// its cached identity (H4 / plan Fase 4).
    pub registration_id: String,
    /// The app-owned listener that accepted this connection, when there is one
    /// (`io.stream-info.listener`).
    pub listener: Option<String>,
    /// The datagram protocol the host attributed to the socket (`udp`, `raw`
    /// or `icmp`); `None` for a byte stream.
    pub protocol: Option<String>,
    /// Effective local endpoint: `"address:port"` for UDP, a bare address for
    /// raw and ICMP.
    pub local_addr: Option<String>,
    /// Effective remote endpoint (the connected UDP peer, or the sender).
    pub remote_addr: Option<String>,
    /// Effective local port when the socket or listener was bound to an
    /// inferred one.
    pub port: Option<u16>,
}

impl StreamInfo {
    /// A stream descriptor with no peer information, stamped `now`, with a
    /// freshly generated [`registration_id`](Self::registration_id).
    pub fn new(transport: impl Into<String>) -> Self {
        Self {
            transport: transport.into(),
            peer: String::new(),
            auth_required: false,
            created_at: unix_millis(),
            bytes_received: 0,
            client_identity: None,
            registration_id: new_registration_id(),
            listener: None,
            protocol: None,
            local_addr: None,
            remote_addr: None,
            port: None,
        }
    }

    /// Mark this stream as requiring an authenticated session.
    #[must_use]
    pub const fn auth_required(mut self, required: bool) -> Self {
        self.auth_required = required;
        self
    }

    /// Attach the verified client-certificate identity of the connection, if
    /// the stream arrived over mutual TLS.
    #[must_use]
    pub fn client_identity(mut self, identity: Option<ferris_core::ClientIdentity>) -> Self {
        self.client_identity = identity;
        self
    }

    /// Name the datagram protocol the socket carries (`udp`, `raw`, `icmp`).
    #[must_use]
    pub fn protocol(mut self, protocol: impl Into<String>) -> Self {
        self.protocol = Some(protocol.into());
        self
    }

    /// Record the effective endpoints and local port the host bound.
    #[must_use]
    pub fn endpoints(
        mut self,
        local_addr: Option<String>,
        remote_addr: Option<String>,
        port: Option<u16>,
    ) -> Self {
        self.local_addr = local_addr;
        self.remote_addr = remote_addr;
        self.port = port;
        self
    }
}

/// A fresh opaque stream-registration token.
///
/// Uniqueness is what the guest keys its cache invalidation on, not
/// unpredictability.
///
/// A process-wide atomic counter combined with the epoch millisecond makes each
/// re-registration distinct without pulling in a randomness/crypto dependency
/// (`ferris-wasm` carries none), and it differs from `created_at` even when two
/// re-registrations land in the same millisecond.
pub(crate) fn new_registration_id() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{seq}", unix_millis())
}

/// Unix epoch milliseconds, saturating to 0 before the epoch (never on a
/// sane clock): the `stream-info.created-at` wire value.
pub(crate) fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Project the host-side [`ferris_core::ClientIdentity`] onto the generated WIT
/// record the guest receives. Shared by the stream path (here) and the HTTP
/// request path (the host's `HttpRequest` builder), so both carry the exact
/// same shape.
#[must_use]
pub fn wit_client_identity(
    identity: ferris_core::ClientIdentity,
) -> crate::ferris::framework::types::ClientIdentity {
    crate::ferris::framework::types::ClientIdentity {
        subject: identity.subject,
        common_name: identity.common_name,
        fingerprint: identity.fingerprint,
        issuer: identity.issuer,
    }
}

/// The signed-in user (and roles) the host attached to this request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserContext {
    pub name: String,
    pub email: String,
    pub roles: Vec<String>,
}

/// A registered cron schedule as `scheduler::list-schedules` reports it — the host side
/// of the WIT `scheduler/schedule-info` record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleInfo {
    pub id: String,
    pub job_name: String,
    pub cron: String,
    pub enabled: bool,
}

/// One operator filter bound to a document field's string value — the host
/// side of the WIT `docstore/filter` record, so [`HostApi`] stays free of the
/// generated WIT types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocFilter {
    /// Field name inside the stored document body.
    pub field: String,
    /// "eq" | "ne" | "like" | "in" | "gt" | "gte" | "lt" | "lte" | "between".
    /// `in` and `between` take comma-separated ranges in `value`; `like` is a
    /// case-insensitive substring match.
    pub op: String,
    pub value: String,
}

/// Rich list parameters — the host side of the WIT `docstore/query` record.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListDocsQuery {
    /// Filters, joined with AND. "eq"/"like"/"in"/"gt"/"gte"/"lt"/"lte" are
    /// answered by the host query engine; "ne" and "between" (which the engine
    /// does not push down) are applied by the host after the engine match,
    /// which keeps pagination honest.
    pub filters: Vec<DocFilter>,
    /// (field, "asc"|"desc") sort keys, applied in order.
    pub order_by: Vec<(String, String)>,
    /// Page size; the host caps a missing limit at 1000 rows.
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// Projection: only these fields appear in each returned `doc.data` object
    /// (plus the system fields `name` / `owner` / `docstatus`).
    pub fields: Option<Vec<String>>,
}

/// One aggregation to compute — the host side of the WIT
/// `docstore/aggregate-spec` record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocAggregateSpec {
    /// `"count"` | `"sum"` | `"avg"` | `"min"` | `"max"`.
    pub func: String,
    /// Field to aggregate; ignored by `count`.
    pub field: String,
    /// Result key; `None` falls back to `"<fn>_<field>"` (or `"count"`).
    pub alias: Option<String>,
}

/// One group of an aggregation — the host side of the WIT
/// `docstore/aggregate-row` record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocAggregateRow {
    /// The `group-by` key values, in order.
    pub group: Vec<(String, String)>,
    /// The computed aggregate values, in the order the specs were given.
    pub values: Vec<(String, String)>,
}

/// One write of a `data::apply-ops` batch — the host side of the WIT
/// `data.write` record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocOp {
    /// `"create"` | `"upsert"` | `"update"` | `"delete"`.
    pub kind: String,
    /// Target `DocType`.
    pub doctype: String,
    /// Row name. Required by `update`/`delete`, ignored by `create`/`upsert`
    /// (the body in `data` carries it).
    pub name: String,
    /// JSON body for `create`/`upsert`/`update`; `None` for `delete`.
    pub data: Option<String>,
    /// Whether the app's own handlers re-enter on this write.
    #[serde(default)]
    pub disposition: WriteDisposition,
}

/// Whether the plugin's own `doctype-event` handlers re-enter on a write.
///
/// `data.write-disposition`: `silent` persists and stamps the row but
/// re-enters none of the app's handlers, which is the only way for a plugin
/// reacting to its own document to avoid the reentrancy loop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WriteDisposition {
    /// Run the normal `before-*` validation and queue the `after-*` events.
    #[default]
    EmitEvents,
    /// Persist and stamp the row, re-entering nothing.
    Silent,
}

/// What a signed certificate is for — mirror of `platform.cert-usage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertUsage {
    /// A client certificate: usable for mTLS against the host.
    Client,
    /// A server certificate: additionally needs `tls-server`.
    Server,
}

impl CertUsage {
    /// The WIT name, so a host provider can record what it issued.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
        }
    }
}

/// An outbound HTTP call — the host side of the WIT `io.request` record.
#[derive(Clone, Serialize, Deserialize)]
pub struct OutboundHttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout_ms: Option<u32>,
    /// `"limited"` (default) | `"none"` | `"follow"`; see the WIT doc.
    pub redirect_policy: Option<String>,
    /// Hosts the guest narrowed the call to (exact host or `"*.suffix"`). The
    /// intersection with the operator allow-list is what applies; an empty
    /// value means "no narrowing", never "any host".
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// Whether loopback/RFC 1918/link-local targets are reachable for this
    /// call. Only meaningful with `http-unrestricted`: the operator allow-list
    /// already decides for a sandboxed app, and a name that resolves to a
    /// private address must not sneak past it.
    #[serde(default)]
    pub allow_private: bool,
    /// Per-call body cap from the guest policy, already in bytes. The host
    /// takes the strictest of this, the app default and its own ceiling.
    #[serde(default)]
    pub max_body: Option<u64>,
    /// Redirect hops the guest asked for (`limited(n)`); the host clamps it to
    /// its own cap.
    #[serde(default)]
    pub redirect_hops: Option<u8>,
    /// Whether `Authorization`/`Cookie` are forwarded. Only an app holding
    /// `http-unrestricted` may ask; the bridge refuses it otherwise.
    #[serde(default)]
    pub allow_credentials: bool,
    /// The call was made under `http-unrestricted`: the allow-list and
    /// `allow-private` are skipped, every size/time/redirect cap still applies.
    #[serde(default)]
    pub unrestricted: bool,
    /// Outbound TLS, with its PEM material already resolved from config.
    #[serde(default)]
    pub tls: Option<OutboundTls>,
}

/// Outbound TLS of one call (or the app default), material resolved to PEM.
///
/// The bridge, not this record, resolves `tls-material.secret` against the
/// app's config store: reading a secret is a capability check, and the record
/// is passed to a transport that must not need database access.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundTls {
    /// Client certificate chain, PEM.
    pub cert: Option<String>,
    /// Client private key, PEM. Never logged, never projected.
    pub key: Option<String>,
    /// CA bundle that verifies the peer, PEM. `None` keeps the system roots.
    pub ca: Option<String>,
    /// SNI override; `None` uses the URL host.
    pub server_name: Option<String>,
    /// ALPN protocols to offer, in order.
    pub alpn: Vec<String>,
    /// Minimum protocol version.
    pub min_version: TlsProtocol,
    /// Verify the peer certificate and chain. `false` needs `tls-client` and
    /// `http-unrestricted`.
    pub verify: bool,
}

impl std::fmt::Debug for OutboundTls {
    /// Redacts every piece of key material: a TLS policy reaches the log
    /// through error paths, and a private key in a log line is a leak.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboundTls")
            .field("cert", &self.cert.as_ref().map(|_| "<pem>"))
            .field("key", &self.key.as_ref().map(|_| "<redacted>"))
            .field("ca", &self.ca.as_ref().map(|_| "<pem>"))
            .field("server_name", &self.server_name)
            .field("alpn", &self.alpn)
            .field("min_version", &self.min_version)
            .field("verify", &self.verify)
            .finish()
    }
}

#[allow(clippy::missing_fields_in_debug)] // the body is reported as a length on purpose
impl std::fmt::Debug for OutboundHttpRequest {
    /// Hand-written so the client key of an outbound TLS policy can never be
    /// printed by a `{:?}` in a log line or an error message, and so the body
    /// is reported as a length instead of its bytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutboundHttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &self.headers)
            .field("body_len", &self.body.as_ref().map(Vec::len))
            .field("timeout_ms", &self.timeout_ms)
            .field("redirect_policy", &self.redirect_policy)
            .field("allowed_hosts", &self.allowed_hosts)
            .field("allow_private", &self.allow_private)
            .field("max_body", &self.max_body)
            .field("allow_credentials", &self.allow_credentials)
            .field("unrestricted", &self.unrestricted)
            .field("tls", &self.tls)
            .finish()
    }
}

/// TLS protocol floor, mirror of WIT `types.tls-version`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TlsProtocol {
    /// TLS 1.2 and 1.3.
    #[default]
    Tls12,
    /// TLS 1.3 only.
    Tls13,
}

/// An outbound HTTP response — the host side of the WIT `io.response`
/// record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundHttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// The transport of a listener or an outbound connection, host-side.
///
/// Named `ListenerTransport` rather than `StreamTransport` because the WIT
/// `types.stream-transport` is re-exported under that name at the crate root;
/// the two must not shadow each other. The three datagram variants travel here
/// too: `io::connect-stream`/`io::listen-stream` refuse them (the datagram
/// entry points are `udp-*`/`raw-open`), but the wire enum is shared and a
/// silent coercion would be worse than the refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerTransport {
    /// An HTTP upgrade.
    Websocket,
    /// A raw byte stream.
    Tcp,
    /// A raw byte stream the host wraps in TLS.
    Tls,
    /// Connected or listening UDP datagrams.
    Udp,
    /// Whole IP packets the guest builds.
    Raw,
    /// ICMP echo traffic.
    Icmp,
}

impl ListenerTransport {
    /// Whether this transport carries whole datagrams (`handle-datagram`)
    /// rather than a byte stream (`handle-stream`).
    #[must_use]
    pub const fn is_datagram(self) -> bool {
        matches!(self, Self::Udp | Self::Raw | Self::Icmp)
    }
}

impl std::fmt::Display for ListenerTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Websocket => "websocket",
            Self::Tcp => "tcp",
            Self::Tls => "tls",
            Self::Udp => "udp",
            Self::Raw => "raw",
            Self::Icmp => "icmp",
        })
    }
}

/// A listener the app asked for: the host side of `types.stream-listen`.
///
/// The declaration is carried **verbatim**, including the TLS material variant
/// (`pem`/`secret`) and the auth policy: resolving a `secret` needs the app's
/// config store and its `secrets` grant, which the socket owner (the server's
/// stream transport) has and this record — a snapshot of `describe()` — does
/// not. Keeping it unresolved here is what lets the same value feed both a
/// declared stream route and an imperative `io::listen-stream`.
#[derive(Debug, Clone)]
pub struct StreamListenerSpec {
    /// `websocket` | `tcp` | `tls` (datagram transports are refused).
    pub transport: ListenerTransport,
    /// The path prefix under the app (`websocket`) or the bind host
    /// (`tcp`/`tls`); `None` means the host infers it. An `"host:port"` inside
    /// the value is still honoured for compatibility.
    pub address: Option<String>,
    /// The port to bind; `None`/`0` lets the host pick one, reported by
    /// `stream-describe`.
    pub port: Option<u16>,
    /// Server material and policy for a `tls` listener.
    pub tls: Option<crate::ferris::framework::types::TlsServer>,
    /// Authentication the host enforces before accepting a connection.
    pub auth: Option<crate::ferris::framework::types::RouteAuth>,
    /// Cap on one chunk forwarded to the guest.
    pub max_frame: Option<u32>,
    /// Idle timeout of one connection.
    pub idle_timeout_seconds: Option<u32>,
}

/// An outbound stream the guest asked for: the host side of
/// `io.connect-spec`.
///
/// Like [`StreamListenerSpec`], the TLS policy travels as declared: the
/// transport resolves `secret` material against the app's config.
#[derive(Debug, Clone)]
pub struct OutboundStreamSpec {
    /// `websocket` | `tcp` | `tls` (datagram transports are refused).
    pub transport: ListenerTransport,
    /// `tcp://host:port`, `tls://host:port`, `ws://host/path`, `wss://host/path`.
    pub uri: String,
    /// Target host; takes precedence over the authority inside `uri` for
    /// `tcp`/`tls`. `None` means inferred from the URI or the scheme default.
    pub address: Option<String>,
    /// Target port; takes precedence over the URI's port. `None`/`0` means
    /// inferred from the URI or the scheme default.
    pub port: Option<u16>,
    /// TLS policy of the connection, falling back to the app default.
    pub tls: Option<crate::ferris::framework::types::TlsClient>,
    /// The outbound policy of the call (allow-list, private, caps).
    pub policy: Option<OutboundPolicy>,
    /// Cap on one chunk forwarded to the guest.
    pub max_frame: Option<u32>,
    /// Idle timeout of the connection.
    pub idle_timeout_seconds: Option<u32>,
}

/// The outbound policy the transport enforces on a stream (the same shape the
/// HTTP path uses).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutboundPolicy {
    /// Hosts the call is narrowed to (empty: no narrowing).
    pub allowed_hosts: Vec<String>,
    /// Whether loopback/RFC 1918/link-local targets are reachable.
    pub allow_private: bool,
    /// Per-call body/chunk cap.
    pub max_body: Option<u64>,
    /// Whether the call ignores the allow-list (`http-unrestricted`).
    pub unrestricted: bool,
}

// -- datagram sockets -------------------------------------------------------

/// One connected UDP socket the host holds: the host side of `io.udp-spec`.
///
/// The guest never sees the descriptor: it names the peer and the bind policy,
/// and every inbound datagram reaches `handle-datagram` for the returned
/// `stream-id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpSpec {
    /// The peer the socket is connected to.
    pub address: String,
    /// Peer port; `None`/`0` means the resolving transport infers it.
    pub port: Option<u16>,
    /// Local bind address; `None` = the host's bind policy.
    pub local_address: Option<String>,
    /// Local bind port; `None`/`0` = an ephemeral port reported by
    /// `stream-describe`.
    pub local_port: Option<u16>,
    /// Allow datagrams to a broadcast address.
    pub broadcast: bool,
    /// IP TTL; `None` = the operating system's default.
    pub ttl: Option<u8>,
}

/// A UDP listener the host binds and the app owns: the host side of
/// `io.udp-listener`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpListenerSpec {
    /// Local bind address; `None` = the host's bind policy (loopback first).
    pub address: Option<String>,
    /// Local port; `None`/`0` = inferred, reported by `stream-describe`.
    pub port: Option<u16>,
    /// Allow broadcast traffic.
    pub broadcast: bool,
    /// Largest datagram forwarded in one call, clamped by the host's own cap.
    pub max_datagram: Option<u32>,
}

/// The datagram transports a raw socket may carry.
///
/// Narrower than [`ListenerTransport`] on purpose: a `raw`/`icmp` request with
/// another transport is `validation-failed`, never a coerced socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawTransport {
    /// Whole IP packets the guest builds.
    Raw,
    /// ICMP echo traffic.
    Icmp,
}

impl std::fmt::Display for RawTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Raw => "raw",
            Self::Icmp => "icmp",
        })
    }
}

/// A raw/ICMP socket the host opens: the host side of `io.raw-spec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSpec {
    /// `raw` or `icmp`.
    pub transport: RawTransport,
    /// Local bind address; `None` = the host's bind policy (loopback first).
    pub address: Option<String>,
    /// Local port; raw and ICMP sockets have no port of their own.
    pub port: Option<u16>,
    /// The IP protocol number the socket receives; `None` = every protocol the
    /// host allows.
    pub ip_protocol: Option<u8>,
}

// -- authoritative DNS ------------------------------------------------------

/// The record types an authoritative zone may carry (`io.dns-record-type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsRecordType {
    A,
    Aaaa,
    Cname,
    Mx,
    Txt,
    Srv,
    Ptr,
    Ns,
    Soa,
}

impl DnsRecordType {
    /// The lowercase wire name, for logs and the encoders.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::Aaaa => "aaaa",
            Self::Cname => "cname",
            Self::Mx => "mx",
            Self::Txt => "txt",
            Self::Srv => "srv",
            Self::Ptr => "ptr",
            Self::Ns => "ns",
            Self::Soa => "soa",
        }
    }
}

/// One resource record: the host side of `io.dns-record`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsRecord {
    /// Relative to the zone's `origin` (`""` and `"@"` are the apex).
    pub name: String,
    /// The record's type.
    pub record_type: DnsRecordType,
    /// Seconds; `0` means the host's default.
    pub ttl: u32,
    /// RDATA in textual zone-file form.
    pub data: String,
    /// MX preference or SRV priority; ignored by every other type.
    pub priority: Option<u16>,
}

/// The typed SOA of a zone: the host side of `io.dns-soa`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsSoa {
    pub primary_ns: String,
    pub admin: String,
    /// `0` makes the host derive one from the records.
    pub serial: u32,
    pub refresh: u32,
    pub retry: u32,
    pub expire: u32,
    pub minimum_ttl: u32,
}

/// One authoritative zone and where the host binds it: the host side of
/// `io.dns-zone`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsZone {
    /// The zone apex, e.g. `example.test.`.
    pub origin: String,
    /// Bind address; `None` = the host's bind policy.
    pub address: Option<String>,
    /// Bind port; `None` = 53 when the host may bind it (otherwise inferred),
    /// `0` = always inferred.
    pub port: Option<u16>,
    pub records: Vec<DnsRecord>,
    /// The typed SOA; declaring it *and* a `soa` record is `validation-failed`.
    pub soa: Option<DnsSoa>,
}

// -- host programs ----------------------------------------------------------

/// One program to run on the app's behalf: the host side of
/// `platform.process-spec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    /// Executable name or a path resolved inside the app's world.
    pub program: String,
    /// Arguments, passed verbatim (there is no shell).
    pub args: Vec<String>,
    /// The whole environment; the host adds only its fixed baseline.
    pub env: Vec<(String, String)>,
    /// Working directory relative to `/world`; may not leave it.
    pub cwd: Option<String>,
    /// Bytes written once and the pipe closed.
    pub stdin: Option<Vec<u8>>,
    /// Deadline; `None`/`0` means no deadline of the guest's.
    pub timeout_ms: Option<u32>,
}

/// What a finished process reported: the host side of
/// `platform.process-output`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Whether the host killed the run for exceeding `timeout-ms`.
    pub timed_out: bool,
}

/// `DocType` field metadata — the host side of the WIT `schema/field-meta`
/// record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaFieldMeta {
    pub name: String,
    pub fieldtype: String,
    pub label: String,
    pub required: bool,
    pub read_only: bool,
    pub options: Option<String>,
}

/// `DocType` metadata for introspection — the host side of the WIT
/// `schema/doctype-meta` record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaDoctypeMeta {
    pub name: String,
    pub fields: Vec<SchemaFieldMeta>,
    pub is_submittable: bool,
    pub is_child_table: bool,
    pub is_single: bool,
}
