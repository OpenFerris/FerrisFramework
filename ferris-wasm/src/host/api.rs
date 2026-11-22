//! The services the framework offers to plugins.

use anyhow::Result;
use async_trait::async_trait;

use super::error::BridgeError;
use super::types::{
    CertUsage, DnsZone, DocAggregateRow, DocAggregateSpec, DocOp, DocRecord, ListDocsQuery,
    OutboundHttpRequest, OutboundHttpResponse, OutboundStreamSpec, ProcessOutput, ProcessSpec,
    RawSpec, ScheduleInfo, SchemaDoctypeMeta, StreamInfo, StreamListenerSpec, UdpListenerSpec,
    UdpSpec, UserContext,
};

/// The services the framework offers to plugins. This is the host side of the
/// WIT imports; `ferris-api`/`ferris-document`/`ferris-files` implement it.
///
/// Implementations must be cheap: the call already lives inside the shared
/// runtime, so no extra runtime or spawn machinery is needed here.
#[async_trait]
pub trait HostApi: Send + Sync + 'static {
    // -- docstore -----------------------------------------------------------
    async fn docstore_list(&self, doctype: &str, query: &ListDocsQuery) -> Result<Vec<DocRecord>>;
    async fn docstore_get(&self, doctype: &str, name: &str) -> Result<DocRecord>;
    /// Fetch many documents by name in one call. The result follows `names`
    /// order and omits names that do not exist (unlike `docstore_get`).
    async fn docstore_get_many(&self, doctype: &str, names: &[String]) -> Result<Vec<DocRecord>>;
    async fn docstore_create(&self, doctype: &str, data: &str) -> Result<DocRecord>;
    /// Create-or-update by the `name` in `data` (idempotent upsert).
    async fn docstore_upsert(&self, doctype: &str, data: &str) -> Result<DocRecord>;
    /// Upsert that suppresses the plugin `doctype-event` forwarder: the row is
    /// written and stamped normally, but no guest handler re-enters on it.
    async fn docstore_write_no_events(&self, doctype: &str, data: &str) -> Result<DocRecord>;
    async fn docstore_update(&self, doctype: &str, name: &str, data: &str) -> Result<DocRecord>;
    async fn docstore_delete(&self, doctype: &str, name: &str) -> Result<()>;
    /// Count the documents matching `query` without materialising the rows.
    async fn docstore_count(&self, doctype: &str, query: &ListDocsQuery) -> Result<u64>;
    /// Group and aggregate the documents matching `query` (bounded scan).
    async fn docstore_aggregate(
        &self,
        doctype: &str,
        query: &ListDocsQuery,
        group_by: &[String],
        aggregates: &[DocAggregateSpec],
    ) -> Result<Vec<DocAggregateRow>>;
    async fn docstore_bulk_create(
        &self,
        doctype: &str,
        records: &[String],
    ) -> Result<Vec<DocRecord>>;
    /// Apply a batch of writes (`upsert`/`update`/`delete`) as **one**
    /// transaction, returning the documents written in op order (`delete` ops
    /// contribute none). Any failing op rolls the whole batch back.
    async fn docstore_apply(&self, ops: &[DocOp]) -> Result<Vec<DocRecord>>;
    /// Rename a document's primary key.
    async fn docstore_rename(&self, doctype: &str, name: &str, new_name: &str)
    -> Result<DocRecord>;
    /// Move a document through `submit`/`cancel`/`amend`.
    ///
    /// `action` is the WIT spelling (`submit`, `cancel`, `amend`); an unknown
    /// action is a validation error, never a silent no-op.
    async fn docstore_set_docstatus(
        &self,
        doctype: &str,
        name: &str,
        action: &str,
    ) -> Result<DocRecord>;
    /// The complete DocType metadata as the JSON `GET /api/doctype/meta` serves.
    async fn docstore_doctype_json(&self, doctype: &str) -> Result<String>;
    /// The canonical fieldtype vocabulary, sorted.
    async fn field_types(&self) -> Vec<String>;

    // -- pubsub -------------------------------------------------------------
    async fn pubsub_publish(&self, channel: &str, event_type: &str, payload: &str) -> Result<()>;

    // -- notification -------------------------------------------------------
    async fn notification_send(
        &self,
        to: &str,
        channel: &str,
        title: &str,
        message: &str,
    ) -> Result<()>;

    // -- identity -----------------------------------------------------------
    async fn naming_next(&self, series: &str) -> Result<String>;
    async fn session_current(&self) -> Result<UserContext>;
    /// Ask whether the (ambient) user may perform `action` on `doctype`.
    ///
    /// With `name == None` this is the DocType-wide check (any row). With a
    /// `name` it is the row-level check: the host resolves the row's owner and
    /// evaluates the policy against it, so `is_owner` grants fire.
    async fn session_can(&self, doctype: &str, action: &str, name: Option<&str>) -> Result<bool>;
    /// Batch form of [`session_can`](Self::session_can): one boolean per check,
    /// in input order.
    async fn session_can_many(
        &self,
        checks: &[(String, String, Option<String>)],
    ) -> Result<Vec<bool>>;

    // -- scheduler ----------------------------------------------------------
    /// Register the calling plugin's cron schedule; returns its id.
    async fn scheduler_schedule(&self, plugin: &str, record: &str) -> Result<String>;
    async fn scheduler_enqueue(&self, plugin: &str, job_type: &str, payload: &str) -> Result<()>;
    /// Remove one of the calling plugin's schedules.
    async fn scheduler_cancel(&self, plugin: &str, schedule_id: &str) -> Result<()>;
    /// The calling plugin's schedules, as `(id, job_name, cron, enabled)`.
    async fn scheduler_list(&self, plugin: &str) -> Result<Vec<ScheduleInfo>>;

    // -- streams ------------------------------------------------------------
    async fn streams_info(&self, stream_id: &str) -> Result<StreamInfo>;
    /// The stream's carry-over buffer (empty when the guest set none).
    async fn streams_state(&self, stream_id: &str) -> Result<Vec<u8>>;
    /// Replace the stream's carry-over buffer.
    async fn streams_set_state(&self, stream_id: &str, pending: &[u8]) -> Result<()>;
    /// Push `bytes` to the peer of a live stream (`io::send-stream`).
    ///
    /// Defaults to [`BridgeError::Unimplemented`]: writing to a live socket is
    /// the transport's job, and the runtime has no transport of its own.
    /// `app` scopes the write: a plugin may only write to a stream it owns.
    async fn streams_send(&self, app: &str, stream_id: &str, bytes: &[u8]) -> Result<()> {
        let _ = (app, stream_id, bytes);
        Err(BridgeError::unimplemented(
            "io::send-stream (no stream transport in this runtime)",
        ))
    }
    /// Close a live stream or stop accepting on a listener
    /// (`io::close-stream`). Defaults to [`BridgeError::Unimplemented`] for the
    /// same reason as [`streams_send`](Self::streams_send).
    async fn streams_close(&self, app: &str, stream_id: &str) -> Result<()> {
        let _ = (app, stream_id);
        Err(BridgeError::unimplemented(
            "io::close-stream (no stream transport in this runtime)",
        ))
    }

    /// Open an outbound stream for `app` (`io::connect-stream`) and return its
    /// host-assigned id.
    ///
    /// The sockets live in the running server, so the default refuses with
    /// [`BridgeError::Unimplemented`] rather than pretending a connection
    /// exists; a host with a transport overrides it.
    ///
    /// # Errors
    ///
    /// [`BridgeError::Unimplemented`] when this runtime owns no sockets, or the
    /// transport's own failure.
    async fn streams_connect(&self, app: &str, spec: &OutboundStreamSpec) -> Result<String> {
        let _ = (app, spec);
        Err(BridgeError::unimplemented(
            "io::connect-stream (no stream transport in this runtime)",
        ))
    }

    /// Bind a listener for `app` (`io::listen-stream`) and return its id.
    ///
    /// Defaults to [`BridgeError::Unimplemented`], like
    /// [`streams_connect`](Self::streams_connect).
    ///
    /// # Errors
    ///
    /// [`BridgeError::Unimplemented`] when this runtime owns no listener, or the
    /// transport's own failure.
    async fn streams_listen(&self, app: &str, spec: &StreamListenerSpec) -> Result<String> {
        let _ = (app, spec);
        Err(BridgeError::unimplemented(
            "io::listen-stream (no stream transport in this runtime)",
        ))
    }

    // -- datagram sockets ---------------------------------------------------
    //
    // The host owns every datagram socket exactly as it owns every byte
    // stream: the guest names the peer or the bind policy and receives whole
    // datagrams in `handle-datagram`. Opening, writing and closing one is the
    // running server's job, so the defaults refuse with the typed
    // `unsupported` failure instead of pretending a socket exists; a host with
    // a datagram transport overrides them. `app` scopes every operation to the
    // calling plugin, so one app can never write to another app's socket.

    /// Open a connected UDP socket for `app` (`io::udp-connect`) and return its
    /// host-assigned `stream-id`.
    async fn udp_connect(&self, app: &str, spec: &UdpSpec) -> Result<String> {
        let _ = (app, spec);
        Err(BridgeError::unsupported(
            "io::udp-connect (no datagram transport in this runtime)",
        ))
    }

    /// Bind a UDP listener for `app` (`io::udp-listen`) and return its id.
    async fn udp_listen(&self, app: &str, spec: &UdpListenerSpec) -> Result<String> {
        let _ = (app, spec);
        Err(BridgeError::unsupported(
            "io::udp-listen (no datagram transport in this runtime)",
        ))
    }

    /// Send one datagram on a socket or listener (`io::udp-send`); `peer`
    /// overrides the destination for a connected socket and is required for a
    /// listener.
    async fn udp_send(
        &self,
        app: &str,
        stream_id: &str,
        datagram: &[u8],
        peer: Option<&str>,
    ) -> Result<()> {
        let _ = (app, stream_id, datagram, peer);
        Err(BridgeError::unsupported(
            "io::udp-send (no datagram transport in this runtime)",
        ))
    }

    /// Close a UDP socket or stop accepting on a UDP listener. Idempotent by
    /// contract: an unknown id is a no-op.
    async fn udp_close(&self, app: &str, stream_id: &str) -> Result<()> {
        let _ = (app, stream_id);
        Err(BridgeError::unsupported(
            "io::udp-close (no datagram transport in this runtime)",
        ))
    }

    /// Open a raw/ICMP socket for `app` (`io::raw-open`) and return its id.
    async fn raw_open(&self, app: &str, spec: &RawSpec) -> Result<String> {
        let _ = (app, spec);
        Err(BridgeError::unsupported(
            "io::raw-open (no raw-socket transport in this runtime)",
        ))
    }

    /// Send one guest-built packet (`io::raw-send`); `destination` overrides
    /// the peer the packet goes to.
    async fn raw_send(
        &self,
        app: &str,
        stream_id: &str,
        packet: &[u8],
        destination: Option<&str>,
    ) -> Result<()> {
        let _ = (app, stream_id, packet, destination);
        Err(BridgeError::unsupported(
            "io::raw-send (no raw-socket transport in this runtime)",
        ))
    }

    /// Close a raw socket. Idempotent by contract.
    async fn raw_close(&self, app: &str, stream_id: &str) -> Result<()> {
        let _ = (app, stream_id);
        Err(BridgeError::unsupported(
            "io::raw-close (no raw-socket transport in this runtime)",
        ))
    }

    /// Serve `zone` authoritatively (`io::dns-serve`) and return the listener
    /// id. The host binds UDP and TCP on the same port.
    async fn dns_serve(&self, app: &str, zone: &DnsZone) -> Result<String> {
        let _ = (app, zone);
        Err(BridgeError::unsupported(
            "io::dns-serve (no DNS transport in this runtime)",
        ))
    }

    /// Replace the records of a live DNS listener atomically (`io::dns-update`).
    async fn dns_update(&self, app: &str, listener_id: &str, zone: &DnsZone) -> Result<()> {
        let _ = (app, listener_id, zone);
        Err(BridgeError::unsupported(
            "io::dns-update (no DNS transport in this runtime)",
        ))
    }

    /// Stop answering for a DNS listener. Idempotent by contract.
    async fn dns_stop(&self, app: &str, listener_id: &str) -> Result<()> {
        let _ = (app, listener_id);
        Err(BridgeError::unsupported(
            "io::dns-stop (no DNS transport in this runtime)",
        ))
    }

    // -- host programs ------------------------------------------------------
    //
    // The program itself runs as a child of the host inside the app's world:
    // the runtime has no fork, no pipe and no deadline of its own, so the
    // defaults refuse with `unsupported` and the server (which owns the
    // process supervisor) overrides them. A host that never wires them keeps
    // answering the truth; `platform::features` reports `process` only when it
    // can.

    /// Run `spec` to completion and return its captured output.
    async fn process_run(&self, app: &str, spec: &ProcessSpec) -> Result<ProcessOutput> {
        let _ = (app, spec);
        Err(BridgeError::unsupported(
            "platform::process-run (no process supervisor in this runtime)",
        ))
    }

    /// Start `spec` without waiting and return its id.
    async fn process_spawn(&self, app: &str, spec: &ProcessSpec) -> Result<String> {
        let _ = (app, spec);
        Err(BridgeError::unsupported(
            "platform::process-spawn (no process supervisor in this runtime)",
        ))
    }

    /// Wait for a spawned process, handing its output over once.
    async fn process_wait(&self, app: &str, process_id: &str) -> Result<ProcessOutput> {
        let _ = (app, process_id);
        Err(BridgeError::unsupported(
            "platform::process-wait (no process supervisor in this runtime)",
        ))
    }

    /// Stop a spawned process and the children the host attributes to it.
    async fn process_kill(&self, app: &str, process_id: &str) -> Result<()> {
        let _ = (app, process_id);
        Err(BridgeError::unsupported(
            "platform::process-kill (no process supervisor in this runtime)",
        ))
    }

    // -- outbound http ------------------------------------------------------
    /// Perform an outbound HTTP call, subject to the host's allow-list, size
    /// caps, timeout and header hygiene.
    async fn http_request(&self, request: &OutboundHttpRequest) -> Result<OutboundHttpResponse>;

    // -- config -------------------------------------------------------------
    /// Read `key` from the calling plugin's namespace.
    async fn config_get(&self, plugin: &str, key: &str) -> Result<Option<String>>;
    async fn config_set(&self, plugin: &str, key: &str, value: &str) -> Result<()>;
    /// Keys of the calling plugin's namespace.
    async fn config_list(&self, plugin: &str) -> Result<Vec<String>>;
    async fn config_delete(&self, plugin: &str, key: &str) -> Result<()>;

    // -- schema -------------------------------------------------------------
    async fn schema_get_doctype(&self, doctype: &str) -> Result<SchemaDoctypeMeta>;
    async fn schema_list_doctypes(&self) -> Result<Vec<String>>;

    // -- pki ----------------------------------------------------------------
    /// Sign a PKCS#10 CSR (DER, base64) with the host's internal CA and return
    /// the issued certificate as PEM. The CA private key stays on the host and
    /// never crosses into the guest. `plugin` is the calling app: the provider
    /// records it as the certificate's issuer-scope.
    async fn pki_sign_csr(
        &self,
        plugin: &str,
        csr_der_b64: &str,
        ttl_seconds: u32,
    ) -> Result<String>;
    /// [`pki_sign_csr`](Self::pki_sign_csr) carrying what the certificate is
    /// for (`platform.sign-csr(usage)`).
    ///
    /// Defaults to the usage-less signature so an existing provider keeps
    /// working; a provider that can distinguish a server certificate overrides
    /// it. `usage = server` additionally requires `tls-server`, which the
    /// bridge checks before calling.
    async fn pki_sign_csr_usage(
        &self,
        plugin: &str,
        csr_der_b64: &str,
        ttl_seconds: u32,
        usage: CertUsage,
    ) -> Result<String> {
        let _ = usage;
        self.pki_sign_csr(plugin, csr_der_b64, ttl_seconds).await
    }
    /// The internal CA certificate in PEM.
    async fn pki_ca_cert(&self) -> Result<String>;
    /// Revoke a certificate by its lowercase hex SHA-256 fingerprint.
    ///
    /// Scoped to the certificates `plugin` issued; another app's fingerprint is
    /// rejected exactly like an unknown one.
    async fn pki_revoke(&self, plugin: &str, fingerprint: &str) -> Result<()>;
    /// Whether a fingerprint is revoked.
    async fn pki_is_revoked(&self, fingerprint: &str) -> Result<bool>;

    /// Attach the ambient user of the current operation to the host, so
    /// plugin `session` imports and host docstore calls resolve the *real*
    /// caller. Synchronous and infallible by design; the default no-op is the
    /// safe choice for hosts that track no identity (they simply report
    /// anonymous and deny).
    fn set_current_user(&self, user: Option<ferris_auth::User>) {
        let _ = user;
    }

    /// Enter/leave **certificate service mode** for the current operation.
    ///
    /// A verified client certificate authenticates a *device*, not a session:
    /// the operation carries `client-identity` but no user. The engine is
    /// default-deny for an anonymous context, so the plugin's own `docstore`
    /// calls on the request path (`hook::on_read_saved` reads `Equipo`) would be
    /// denied before any `DocPerm` rule is consulted (H7). While this flag is
    /// set the host runs docstore operations under an anonymous-but-allowed
    /// service context — the same `ctx.user = None` + `allow_anonymous` pattern
    /// the framework already uses for its system routes
    /// (`ferris-api/src/store.rs::get_single_system`). The plugin is trusted
    /// code, and the *plugin* remains the authority on what the certificate
    /// means; the host only stops default-denying it. Defaults to a no-op.
    fn set_certificate_service(&self, active: bool) {
        let _ = active;
    }

    /// Register a live stream the host is forwarding to `handle-stream`.
    /// Defaults to a no-op; hosts that want `streams::info` to answer must
    /// keep the registry.
    fn register_stream(&self, stream_id: &str, info: StreamInfo) {
        let _ = (stream_id, info);
    }

    /// Forget a closed stream.
    fn unregister_stream(&self, stream_id: &str) {
        let _ = stream_id;
    }

    /// Account `bytes` forwarded into `handle-stream` for `stream_id`, so
    /// `streams::info` can report `bytes-received`. Defaults to a no-op.
    fn stream_received(&self, stream_id: &str, bytes: usize) {
        let _ = (stream_id, bytes);
    }

    /// Client-certificate identity of the connection that registered
    /// `stream_id`, when the host terminated mutual TLS for it.
    ///
    /// Synchronous on purpose: the plugin forwarder consults it while the
    /// guest call is in flight, to enable certificate service mode for a
    /// device-authenticated stream with no session (the guest reads the
    /// docstore on `handle-stream`, e.g. to resolve the raw wire against the
    /// `Sensor` rows). Hosts that do not track identities return `None`.
    fn stream_client_identity(&self, stream_id: &str) -> Option<ferris_core::ClientIdentity> {
        let _ = stream_id;
        None
    }
}
