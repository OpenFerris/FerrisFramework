//! Ferris WASM — shared component runtime for app plugins.
//!
//! A single [`SharedRuntime`] owns one `wasmtime::Engine`, so **every** app
//! plugin shares the same compiler, signal stack arena and tunables. That
//! avoids the cost (and memory) of one runtime per plugin, keeps dispatch
//! overhead flat as plugins grow, and lets the host enforce uniform safety
//! limits (epoch interrupts, memory/stack caps, backtraces off).
//!
//! The app contract lives in `wit/ferris.wit` (`ferris:framework@0.3.0`): six
//! interfaces — `types`, `data`, `schema`, `io`, `platform` and `descriptor` —
//! that apps **import**, plus the nine exports (`describe`, `handle-request`,
//! `doctype-event`, `handle-stream`, `handle-stream-event`, `handle-datagram`,
//! `handle-job`, `ui-asset`, `ui-widget`) the host calls. See [`Plugin`] and
//! [`PluginStore`].
//!
//! The sandbox is explicit: every import goes through a [`Capability`] grant
//! ([`GrantSet`]), and a call outside the effective grant is the typed
//! `permission-denied` variant instead of a silently restricted one. Each app
//! owns a world (`plugins/<app>/`, mounted at `/world`) and an inheritable
//! method chain; the WIT header is the reference.
//!
//! The contract is static even though the host is modular: the running binary
//! says what it actually compiled through [`HostFeatures`]
//! (`platform::features()`), and a call into an absent surface answers the
//! typed `unsupported` failure.

pub mod component;
pub mod descriptor;
pub mod error;
pub mod host;
pub mod host_impl;
pub mod http;
pub mod plugin;
pub mod runtime;
pub(crate) mod setup;
pub mod store;
pub mod tls;

pub use component::{ensure_component, is_component, load_component};
pub use error::PluginErrorKind;
pub use error::WasmError;
pub use host::{
    Capability, CertUsage, DnsRecord, DnsRecordType, DnsSoa, DnsZone, DocAggregateRow,
    DocAggregateSpec, DocFilter, DocOp, DocRecord, GrantSet, HostApi, HostFeatures, HostState,
    ListDocsQuery, ListenerTransport, MethodResolver, NotificationSender, NullHost,
    OutboundHttpRequest, OutboundHttpResponse, OutboundPolicy, OutboundStreamSpec, OutboundTls,
    PkiProvider, ProcessOutput, ProcessService, ProcessSpec, RawSpec, RawTransport,
    RealtimeBroadcaster, ScheduleInfo, SchemaDoctypeMeta, SchemaFieldMeta, StreamInfo,
    StreamListenerSpec, StreamService, TlsProtocol, UdpListenerSpec, UdpSpec, UserContext,
    WriteDisposition, host_features, host_has_feature, method_resolver, notification_sender,
    pki_provider, process_service, realtime_broadcaster, set_method_resolver,
    set_notification_sender, set_pki_provider, set_process_service, set_realtime_broadcaster,
    set_stream_service, stream_service, wit_client_identity,
};
pub use host_impl::{RealHostApi, RealHostState};
pub use http::sanitize_response_headers;
pub use plugin::{PLUGIN_ROUTE_ID, Plugin};
pub use runtime::{RuntimeConfig, SharedRuntime};
pub use store::PluginStore;

// Bindings generated from `wit/ferris.wit` at compile time. The world
// (`ferris-app`) defines the exports every plugin must provide and the
// imports the host must satisfy.
wasmtime::component::bindgen!({
    path: "wit/ferris.wit",
    world: "ferris-app",
});

// The world struct (`FerrisApp`) is emitted at the crate root; the record types
// and per-interface `Host` import traits live under the WIT package namespace.
pub use crate::descriptor::MethodSpec;
pub use crate::ferris::framework::data::{
    AggregateRow, AggregateSpec, Doc, DoctypeMeta, FieldMeta, Filter, OpKind, Query, Sort, Write,
};
pub use crate::ferris::framework::io::{ConnectSpec, Request, StreamInfo as WitStreamInfo};
pub use crate::ferris::framework::platform::User;
pub use crate::ferris::framework::types::{
    ClientAuth, ClientIdentity, HttpPolicy, RedirectPolicy, RouteAuth, StreamListen,
    StreamTransport, TlsClient, TlsMaterial, TlsServer, TlsVersion,
};
// `HttpRequest` and `Response` are emitted at the crate root by `bindgen!`
// (the world `use`s both), so they are already `ferris_wasm::{HttpRequest,
// Response}` — no re-export needed. The same holds for `StreamEvent` and
// `Error` (the world's `use types.{error, http-request, stream-event}`).
