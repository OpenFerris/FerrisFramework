//! Host-facing contract between the component runtime and the framework
//! services: the [`HostApi`] plugins call, the [`HostState`] the linker talks
//! to, and the wire records both exchange.
//!
//! The WIT import bridges live in `imports/`, one file per interface; each
//! forwards into [`HostApi`], the only extension point a host implements (see
//! [`NullHost`] for the rejecting default).
//!
//! [`HostFeatures`] is the host's side of `platform::features()`: the set of
//! verticals the running binary was compiled with, installed once at boot.

mod api;
mod conversions;
mod error;
mod features;
mod globals;
mod grants;
mod imports;
mod null;
mod state;
mod types;

pub use api::HostApi;
pub(crate) use error::to_wit_error;
pub use error::{BridgeError, HostError, classify_host_error};
pub use features::{HostFeatures, host_features, host_has_feature};
pub use globals::{
    MethodResolver, NotificationSender, PkiProvider, ProcessService, RealtimeBroadcaster,
    StreamService, method_resolver, notification_sender, pki_provider, process_service,
    realtime_broadcaster, set_method_resolver, set_notification_sender, set_pki_provider,
    set_process_service, set_realtime_broadcaster, set_stream_service, stream_service,
};
pub use grants::{ApprovedGrants, Capability, GrantSet};
pub use null::NullHost;
pub use state::HostState;
pub use types::{
    CertUsage, DnsRecord, DnsRecordType, DnsSoa, DnsZone, DocAggregateRow, DocAggregateSpec,
    DocFilter, DocOp, DocRecord, ListDocsQuery, ListenerTransport, OutboundHttpRequest,
    OutboundHttpResponse, OutboundPolicy, OutboundStreamSpec, OutboundTls, ProcessOutput,
    ProcessSpec, RawSpec, RawTransport, ScheduleInfo, SchemaDoctypeMeta, SchemaFieldMeta,
    StreamInfo, StreamListenerSpec, TlsProtocol, UdpListenerSpec, UdpSpec, UserContext,
    WriteDisposition, wit_client_identity,
};
pub(crate) use types::{new_registration_id, unix_millis};
