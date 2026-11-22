//! Conversions between the host records and the WIT records.
//!
//! The host keeps its own records on purpose: [`HostApi`](super::HostApi) is
//! free of the generated bindings, so a WIT bump touches this file plus the
//! bridges instead of every service implementation. Every conversion lives
//! here so a new field is a single compile error, not a silently dropped value.

use super::types::{
    CertUsage, DnsRecord, DnsRecordType, DnsSoa, DnsZone, DocAggregateRow, DocAggregateSpec,
    DocFilter, DocOp, DocRecord, ListDocsQuery, ListenerTransport, OutboundHttpRequest,
    OutboundHttpResponse, OutboundPolicy, OutboundStreamSpec, ProcessSpec, RawSpec, RawTransport,
    StreamInfo, StreamListenerSpec, UdpListenerSpec, UdpSpec, UserContext, WriteDisposition,
};
use crate::ferris::framework::types as wit_types;
use crate::ferris::framework::{data as wit_data, io as wit_io, platform as wit_platform};

// -- documents --------------------------------------------------------------

impl From<wit_data::Doc> for DocRecord {
    fn from(doc: wit_data::Doc) -> Self {
        Self {
            name: doc.name,
            doctype: doc.doctype,
            owner: doc.owner,
            docstatus: doc.docstatus,
            data: doc.data,
        }
    }
}

impl From<DocRecord> for wit_data::Doc {
    fn from(doc: DocRecord) -> Self {
        Self {
            name: doc.name,
            doctype: doc.doctype,
            owner: doc.owner,
            docstatus: doc.docstatus,
            data: doc.data,
        }
    }
}

// -- queries ----------------------------------------------------------------

/// The `filter-op` vocabulary the host query engine already understands.
const fn filter_op_name(op: wit_data::FilterOp) -> &'static str {
    match op {
        wit_data::FilterOp::Eq => "eq",
        wit_data::FilterOp::Ne => "ne",
        wit_data::FilterOp::Like => "like",
        wit_data::FilterOp::In => "in",
        wit_data::FilterOp::Gt => "gt",
        wit_data::FilterOp::Gte => "gte",
        wit_data::FilterOp::Lt => "lt",
        wit_data::FilterOp::Lte => "lte",
        wit_data::FilterOp::Between => "between",
    }
}

impl From<wit_data::Filter> for DocFilter {
    fn from(filter: wit_data::Filter) -> Self {
        Self {
            field: filter.field,
            op: filter_op_name(filter.op).to_string(),
            value: filter.value,
        }
    }
}

/// A WIT `data.query` as the host's list parameters.
pub(super) fn list_query_from_wit(query: wit_data::Query) -> ListDocsQuery {
    ListDocsQuery {
        filters: query.filters.into_iter().map(DocFilter::from).collect(),
        order_by: query
            .order_by
            .into_iter()
            .map(|sort| {
                (
                    sort.field,
                    if sort.desc { "desc" } else { "asc" }.to_string(),
                )
            })
            .collect(),
        limit: query.limit,
        offset: query.offset,
        fields: query.fields,
    }
}

// -- writes -----------------------------------------------------------------

/// The host-side name of an `op-kind`, which is also the key
/// [`HostApi::docstore_apply`](super::HostApi::docstore_apply) dispatches on.
pub(super) const fn op_kind_name(kind: wit_data::OpKind) -> &'static str {
    match kind {
        wit_data::OpKind::Create => "create",
        wit_data::OpKind::Update => "update",
        wit_data::OpKind::Upsert => "upsert",
        wit_data::OpKind::Delete => "delete",
    }
}

/// One WIT `data.write` as the host's apply-op record. `name` flattens the
/// WIT `option<string>`; a `delete` keeps its name and drops its body.
pub(super) fn doc_op_from_wit(write: wit_data::Write) -> DocOp {
    DocOp {
        kind: op_kind_name(write.kind).to_string(),
        doctype: write.doctype,
        name: write.name.unwrap_or_default(),
        data: write.data,
        disposition: match write.disposition {
            wit_data::WriteDisposition::EmitEvents => WriteDisposition::EmitEvents,
            wit_data::WriteDisposition::Silent => WriteDisposition::Silent,
        },
    }
}

// -- aggregates -------------------------------------------------------------

/// The host-side name of an `aggregate-fn`.
const fn aggregate_fn_name(function: wit_data::AggregateFn) -> &'static str {
    match function {
        wit_data::AggregateFn::Count => "count",
        wit_data::AggregateFn::Sum => "sum",
        wit_data::AggregateFn::Avg => "avg",
        wit_data::AggregateFn::Min => "min",
        wit_data::AggregateFn::Max => "max",
    }
}

impl From<wit_data::AggregateSpec> for DocAggregateSpec {
    fn from(spec: wit_data::AggregateSpec) -> Self {
        Self {
            func: aggregate_fn_name(spec.fn_).to_string(),
            field: spec.field,
            alias: spec.alias,
        }
    }
}

impl From<DocAggregateRow> for wit_data::AggregateRow {
    fn from(row: DocAggregateRow) -> Self {
        Self {
            group: row.group,
            values: row.values,
        }
    }
}

// -- io ---------------------------------------------------------------------

impl From<OutboundHttpResponse> for wit_io::Response {
    fn from(response: OutboundHttpResponse) -> Self {
        Self {
            status: response.status,
            headers: response.headers,
            body: response.body,
        }
    }
}

impl From<wit_io::Request> for OutboundHttpRequest {
    fn from(request: wit_io::Request) -> Self {
        let policy = request.policy.as_ref();
        Self {
            method: request.method,
            url: request.url,
            headers: request.headers,
            body: request.body,
            // `timeout-ms` on the request is clamped by the host ceiling; the
            // policy's value is the fallback when the call names none.
            timeout_ms: request
                .timeout_ms
                .or_else(|| policy.and_then(|p| p.timeout_ms)),
            redirect_policy: request
                .redirects
                .or_else(|| policy.map(|p| p.redirects))
                .map(|redirects| redirect_policy_name(redirects).to_string()),
            // A guest policy can only narrow: an empty list means "no
            // narrowing", never "any host".
            allowed_hosts: policy.map(|p| p.allowed_hosts.clone()).unwrap_or_default(),
            allow_private: policy.is_some_and(|p| p.allow_private),
            max_body: policy.and_then(|p| p.max_body),
            redirect_hops: request
                .redirects
                .or_else(|| policy.map(|p| p.redirects))
                .and_then(|redirects| match redirects {
                    wit_types::RedirectPolicy::Limited(hops) => Some(hops),
                    _ => None,
                }),
            allow_credentials: policy.is_some_and(|p| p.allow_credentials),
            unrestricted: false,
            // Filled in by the bridge, which has the app identity and the
            // config store needed to resolve `tls-material.secret`.
            tls: None,
        }
    }
}

/// The host-side name of a `redirect-policy`.
const fn redirect_policy_name(policy: wit_types::RedirectPolicy) -> &'static str {
    match policy {
        wit_types::RedirectPolicy::None => "none",
        wit_types::RedirectPolicy::Limited(_) => "limited",
        wit_types::RedirectPolicy::Follow => "follow",
    }
}

impl From<wit_types::StreamTransport> for ListenerTransport {
    fn from(transport: wit_types::StreamTransport) -> Self {
        match transport {
            wit_types::StreamTransport::Websocket => Self::Websocket,
            wit_types::StreamTransport::Tcp => Self::Tcp,
            wit_types::StreamTransport::Tls => Self::Tls,
            wit_types::StreamTransport::Udp => Self::Udp,
            wit_types::StreamTransport::Raw => Self::Raw,
            wit_types::StreamTransport::Icmp => Self::Icmp,
        }
    }
}

impl From<wit_types::StreamListen> for StreamListenerSpec {
    fn from(spec: wit_types::StreamListen) -> Self {
        Self {
            transport: spec.transport.into(),
            address: spec.address,
            port: spec.port,
            tls: spec.tls,
            auth: spec.auth,
            max_frame: spec.max_frame,
            idle_timeout_seconds: spec.idle_timeout_seconds,
        }
    }
}

/// The outbound policy of one call: the intersection of what the guest asked
/// and what the operator granted. `unrestricted` comes from the bridge (the
/// `http-unrestricted` grant), never from the guest.
impl From<&wit_types::HttpPolicy> for OutboundPolicy {
    fn from(policy: &wit_types::HttpPolicy) -> Self {
        Self {
            allowed_hosts: policy.allowed_hosts.clone(),
            allow_private: policy.allow_private,
            max_body: policy.max_body,
            unrestricted: false,
        }
    }
}

/// `io.connect-spec` as the transport's outbound record.
pub(super) fn connect_spec_from_wit(spec: wit_io::ConnectSpec) -> OutboundStreamSpec {
    OutboundStreamSpec {
        transport: spec.transport.into(),
        uri: spec.uri,
        address: spec.address,
        port: spec.port,
        tls: spec.tls,
        policy: spec.policy.as_ref().map(OutboundPolicy::from),
        max_frame: spec.max_frame,
        idle_timeout_seconds: spec.idle_timeout_seconds,
    }
}

/// A live stream as `io.stream-describe` reports it.
impl From<StreamInfo> for wit_io::StreamInfo {
    fn from(info: StreamInfo) -> Self {
        Self {
            transport: info.transport,
            peer: info.peer,
            auth_required: info.auth_required,
            created_at: info.created_at,
            bytes_received: info.bytes_received,
            client_identity: info.client_identity.map(super::types::wit_client_identity),
            registration_id: info.registration_id,
            listener: info.listener,
            protocol: info.protocol,
            local_addr: info.local_addr,
            remote_addr: info.remote_addr,
            port: info.port,
        }
    }
}

impl StreamInfo {
    /// Consume into the WIT `io.stream-info` the guest receives.
    pub(super) fn into_wit(self) -> wit_io::StreamInfo {
        self.into()
    }
}

// -- datagram sockets -------------------------------------------------------

impl From<wit_io::UdpSpec> for UdpSpec {
    fn from(spec: wit_io::UdpSpec) -> Self {
        Self {
            address: spec.address,
            port: spec.port,
            local_address: spec.local_address,
            local_port: spec.local_port,
            broadcast: spec.broadcast,
            ttl: spec.ttl,
        }
    }
}

impl From<wit_io::UdpListener> for UdpListenerSpec {
    fn from(spec: wit_io::UdpListener) -> Self {
        Self {
            address: spec.address,
            port: spec.port,
            broadcast: spec.broadcast,
            max_datagram: spec.max_datagram,
        }
    }
}

/// `io.raw-spec` as the transport's record.
///
/// The wire enum is the shared `stream-transport`, but a raw socket can only
/// be `raw` or `icmp`: any other case is `validation-failed` here rather than a
/// transport silently opening a different kind of socket.
pub(super) fn raw_spec_from_wit(spec: wit_io::RawSpec) -> Result<RawSpec, wit_types::Error> {
    let transport = match spec.transport {
        wit_types::StreamTransport::Raw => RawTransport::Raw,
        wit_types::StreamTransport::Icmp => RawTransport::Icmp,
        other => {
            return Err(wit_types::Error::ValidationFailed(format!(
                "io::raw-open accepts `raw` or `icmp`, not {other:?}"
            )));
        }
    };
    Ok(RawSpec {
        transport,
        address: spec.address,
        port: spec.port,
        ip_protocol: spec.ip_protocol,
    })
}

// -- authoritative DNS ------------------------------------------------------

/// The host-side name of a `dns-record-type`.
const fn dns_record_type(record_type: wit_io::DnsRecordType) -> DnsRecordType {
    match record_type {
        wit_io::DnsRecordType::A => DnsRecordType::A,
        wit_io::DnsRecordType::Aaaa => DnsRecordType::Aaaa,
        wit_io::DnsRecordType::Cname => DnsRecordType::Cname,
        wit_io::DnsRecordType::Mx => DnsRecordType::Mx,
        wit_io::DnsRecordType::Txt => DnsRecordType::Txt,
        wit_io::DnsRecordType::Srv => DnsRecordType::Srv,
        wit_io::DnsRecordType::Ptr => DnsRecordType::Ptr,
        wit_io::DnsRecordType::Ns => DnsRecordType::Ns,
        wit_io::DnsRecordType::Soa => DnsRecordType::Soa,
    }
}

impl From<wit_io::DnsRecord> for DnsRecord {
    fn from(record: wit_io::DnsRecord) -> Self {
        Self {
            name: record.name,
            record_type: dns_record_type(record.type_),
            ttl: record.ttl,
            data: record.data,
            priority: record.priority,
        }
    }
}

impl From<wit_io::DnsSoa> for DnsSoa {
    fn from(soa: wit_io::DnsSoa) -> Self {
        Self {
            primary_ns: soa.primary_ns,
            admin: soa.admin,
            serial: soa.serial,
            refresh: soa.refresh,
            retry: soa.retry,
            expire: soa.expire,
            minimum_ttl: soa.minimum_ttl,
        }
    }
}

impl From<wit_io::DnsZone> for DnsZone {
    fn from(zone: wit_io::DnsZone) -> Self {
        Self {
            origin: zone.origin,
            address: zone.address,
            port: zone.port,
            records: zone.records.into_iter().map(DnsRecord::from).collect(),
            soa: zone.soa.map(DnsSoa::from),
        }
    }
}

// -- host programs ----------------------------------------------------------

impl From<wit_platform::ProcessSpec> for ProcessSpec {
    fn from(spec: wit_platform::ProcessSpec) -> Self {
        Self {
            program: spec.program,
            args: spec.args,
            env: spec.env,
            cwd: spec.cwd,
            stdin: spec.stdin,
            timeout_ms: spec.timeout_ms,
        }
    }
}

impl From<super::types::ProcessOutput> for wit_platform::ProcessOutput {
    fn from(output: super::types::ProcessOutput) -> Self {
        Self {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
            timed_out: output.timed_out,
        }
    }
}

// -- platform ---------------------------------------------------------------

impl From<UserContext> for wit_platform::User {
    fn from(user: UserContext) -> Self {
        Self {
            name: user.name,
            email: user.email,
            roles: user.roles,
        }
    }
}

/// What a signed certificate is for.
pub(super) const fn cert_usage_from_wit(usage: wit_platform::CertUsage) -> CertUsage {
    match usage {
        wit_platform::CertUsage::Client => CertUsage::Client,
        wit_platform::CertUsage::Server => CertUsage::Server,
    }
}

// -- bridge shorthands ------------------------------------------------------
//
// The bridges read as a pipeline (`query.from_wit()`, `doc.into_wit()`) instead
// of importing a `From` impl per record. The conversion itself stays in this
// module, so a WIT field change is still one compile error here.

impl DocRecord {
    /// Consume into the WIT document the guest receives.
    pub(super) fn into_wit(self) -> wit_data::Doc {
        self.into()
    }
}

impl ListDocsQuery {
    /// A WIT `data.query` as the host's list parameters.
    pub(super) fn from_wit(query: wit_data::Query) -> Self {
        list_query_from_wit(query)
    }
}

impl DocAggregateSpec {
    /// A WIT `data.aggregate-spec` as the host's spec record.
    pub(super) fn from_wit(spec: wit_data::AggregateSpec) -> Self {
        spec.into()
    }
}

impl DocOp {
    /// A WIT `data.write` as the host's apply-op record.
    pub(super) fn from_wit(write: wit_data::Write) -> Self {
        doc_op_from_wit(write)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ferris::framework::data as wit;

    fn filter(field: &str, op: wit::FilterOp) -> wit::Filter {
        wit::Filter {
            field: field.to_string(),
            op,
            value: "v".to_string(),
        }
    }

    /// The host query engine speaks the kebab-case names, and a `sort` record
    /// (`desc: bool`) becomes the ("field", "asc"|"desc") pair it expects.
    #[test]
    fn a_wit_query_maps_onto_the_host_list_parameters() {
        let query = wit::Query {
            filters: vec![
                filter("status", wit::FilterOp::Eq),
                filter("n", wit::FilterOp::Gte),
            ],
            order_by: vec![
                wit::Sort {
                    field: "creation".into(),
                    desc: true,
                },
                wit::Sort {
                    field: "name".into(),
                    desc: false,
                },
            ],
            limit: Some(10),
            offset: Some(20),
            fields: Some(vec!["name".into()]),
        };
        let mapped = list_query_from_wit(query);
        assert_eq!(
            mapped
                .filters
                .iter()
                .map(|f| (f.field.as_str(), f.op.as_str()))
                .collect::<Vec<_>>(),
            vec![("status", "eq"), ("n", "gte")]
        );
        assert_eq!(
            mapped.order_by,
            vec![
                ("creation".to_string(), "desc".to_string()),
                ("name".to_string(), "asc".to_string()),
            ]
        );
        assert_eq!(mapped.limit, Some(10));
        assert_eq!(mapped.offset, Some(20));
        assert_eq!(mapped.fields, Some(vec!["name".to_string()]));
    }

    /// `write` flattens `name` and carries the disposition the engine needs to
    /// decide whether the app's handlers re-enter.
    #[test]
    fn a_wit_write_maps_kind_name_and_disposition() {
        let op = DocOp::from_wit(wit::Write {
            kind: wit::OpKind::Upsert,
            doctype: "Task".into(),
            name: None,
            data: Some("{\"name\":\"T-1\"}".into()),
            disposition: wit::WriteDisposition::Silent,
        });
        assert_eq!(op.kind, "upsert");
        assert_eq!(op.name, "");
        assert_eq!(op.disposition, WriteDisposition::Silent);

        let delete = DocOp::from_wit(wit::Write {
            kind: wit::OpKind::Delete,
            doctype: "Task".into(),
            name: Some("T-1".into()),
            data: None,
            disposition: wit::WriteDisposition::EmitEvents,
        });
        assert_eq!(delete.kind, "delete");
        assert_eq!(delete.name, "T-1");
        assert_eq!(delete.disposition, WriteDisposition::EmitEvents);
    }
}
