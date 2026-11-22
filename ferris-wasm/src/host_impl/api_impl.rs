//! The single [`HostApi`] implementation for [`RealHostApi`].
//!
//! Rust allows one trait impl per type, so each interface file (`docstore.rs`,
//! `docstore.rs`, …) holds the inherent methods and this module is the one
//! place that binds them to the trait `Plugin` and the linker talk to. A new
//! service therefore touches its own file plus one forwarding line here.

use super::RealHostApi;
use crate::host::{
    DnsZone, DocAggregateRow, DocAggregateSpec, DocOp, DocRecord, HostApi, ListDocsQuery,
    OutboundHttpRequest, OutboundHttpResponse, OutboundStreamSpec, ProcessOutput, ProcessSpec,
    RawSpec, ScheduleInfo, SchemaDoctypeMeta, StreamInfo, StreamListenerSpec, UdpListenerSpec,
    UdpSpec, UserContext,
};
use anyhow::Result;

#[async_trait::async_trait]
impl HostApi for RealHostApi {
    // -- docstore -----------------------------------------------------------

    async fn docstore_list(&self, doctype: &str, query: &ListDocsQuery) -> Result<Vec<DocRecord>> {
        RealHostApi::docstore_list(self, doctype, query).await
    }

    async fn docstore_get(&self, doctype: &str, name: &str) -> Result<DocRecord> {
        RealHostApi::docstore_get(self, doctype, name).await
    }

    async fn docstore_get_many(&self, doctype: &str, names: &[String]) -> Result<Vec<DocRecord>> {
        RealHostApi::docstore_get_many(self, doctype, names).await
    }

    async fn docstore_create(&self, doctype: &str, data: &str) -> Result<DocRecord> {
        RealHostApi::docstore_create(self, doctype, data).await
    }

    async fn docstore_upsert(&self, doctype: &str, data: &str) -> Result<DocRecord> {
        RealHostApi::docstore_upsert(self, doctype, data).await
    }

    async fn docstore_write_no_events(&self, doctype: &str, data: &str) -> Result<DocRecord> {
        RealHostApi::docstore_write_no_events(self, doctype, data).await
    }

    async fn docstore_update(&self, doctype: &str, name: &str, data: &str) -> Result<DocRecord> {
        RealHostApi::docstore_update(self, doctype, name, data).await
    }

    async fn docstore_delete(&self, doctype: &str, name: &str) -> Result<()> {
        RealHostApi::docstore_delete(self, doctype, name).await
    }

    async fn docstore_count(&self, doctype: &str, query: &ListDocsQuery) -> Result<u64> {
        RealHostApi::docstore_count(self, doctype, query).await
    }

    async fn docstore_aggregate(
        &self,
        doctype: &str,
        query: &ListDocsQuery,
        group_by: &[String],
        aggregates: &[DocAggregateSpec],
    ) -> Result<Vec<DocAggregateRow>> {
        RealHostApi::docstore_aggregate(self, doctype, query, group_by, aggregates).await
    }

    async fn docstore_bulk_create(
        &self,
        doctype: &str,
        records: &[String],
    ) -> Result<Vec<DocRecord>> {
        RealHostApi::docstore_bulk_create(self, doctype, records).await
    }

    async fn docstore_apply(&self, ops: &[DocOp]) -> Result<Vec<DocRecord>> {
        RealHostApi::docstore_apply(self, ops).await
    }

    async fn docstore_rename(
        &self,
        doctype: &str,
        name: &str,
        new_name: &str,
    ) -> Result<DocRecord> {
        RealHostApi::docstore_rename(self, doctype, name, new_name).await
    }

    async fn docstore_set_docstatus(
        &self,
        doctype: &str,
        name: &str,
        action: &str,
    ) -> Result<DocRecord> {
        RealHostApi::docstore_set_docstatus(self, doctype, name, action).await
    }

    async fn docstore_doctype_json(&self, doctype: &str) -> Result<String> {
        RealHostApi::docstore_doctype_json(self, doctype).await
    }

    async fn field_types(&self) -> Vec<String> {
        RealHostApi::field_types()
    }

    // -- pubsub -------------------------------------------------------------

    async fn pubsub_publish(&self, channel: &str, event_type: &str, payload: &str) -> Result<()> {
        RealHostApi::pubsub_publish(self, channel, event_type, payload).await
    }

    // -- notification -------------------------------------------------------

    async fn notification_send(
        &self,
        to: &str,
        channel: &str,
        title: &str,
        message: &str,
    ) -> Result<()> {
        RealHostApi::notification_send(self, to, channel, title, message).await
    }

    // -- identity -----------------------------------------------------------

    async fn naming_next(&self, series: &str) -> Result<String> {
        RealHostApi::naming_next(self, series).await
    }

    async fn session_current(&self) -> Result<UserContext> {
        RealHostApi::session_current(self).await
    }

    async fn session_can(&self, doctype: &str, action: &str, name: Option<&str>) -> Result<bool> {
        RealHostApi::session_can(self, doctype, action, name).await
    }

    async fn session_can_many(
        &self,
        checks: &[(String, String, Option<String>)],
    ) -> Result<Vec<bool>> {
        RealHostApi::session_can_many(self, checks).await
    }

    // -- scheduler ----------------------------------------------------------

    async fn scheduler_schedule(&self, plugin: &str, record: &str) -> Result<String> {
        RealHostApi::scheduler_schedule(self, plugin, record).await
    }

    async fn scheduler_enqueue(&self, plugin: &str, job_type: &str, payload: &str) -> Result<()> {
        RealHostApi::scheduler_enqueue_for(self, plugin, job_type, payload).await
    }

    async fn scheduler_cancel(&self, plugin: &str, schedule_id: &str) -> Result<()> {
        RealHostApi::scheduler_cancel(self, plugin, schedule_id).await
    }

    async fn scheduler_list(&self, plugin: &str) -> Result<Vec<ScheduleInfo>> {
        RealHostApi::scheduler_list(self, plugin).await
    }

    // -- streams ------------------------------------------------------------

    async fn streams_info(&self, stream_id: &str) -> Result<StreamInfo> {
        RealHostApi::streams_info(self, stream_id).await
    }

    async fn streams_state(&self, stream_id: &str) -> Result<Vec<u8>> {
        RealHostApi::streams_state(self, stream_id).await
    }

    async fn streams_set_state(&self, stream_id: &str, pending: &[u8]) -> Result<()> {
        RealHostApi::streams_set_state(self, stream_id, pending).await
    }

    async fn streams_send(&self, app: &str, stream_id: &str, bytes: &[u8]) -> Result<()> {
        RealHostApi::streams_send(self, app, stream_id, bytes).await
    }

    async fn streams_close(&self, app: &str, stream_id: &str) -> Result<()> {
        RealHostApi::streams_close(self, app, stream_id).await
    }

    async fn streams_connect(&self, app: &str, spec: &OutboundStreamSpec) -> Result<String> {
        RealHostApi::streams_connect(self, app, spec).await
    }

    async fn streams_listen(&self, app: &str, spec: &StreamListenerSpec) -> Result<String> {
        RealHostApi::streams_listen(self, app, spec).await
    }

    // -- datagram sockets ---------------------------------------------------

    async fn udp_connect(&self, app: &str, spec: &UdpSpec) -> Result<String> {
        RealHostApi::udp_connect(self, app, spec).await
    }

    async fn udp_listen(&self, app: &str, spec: &UdpListenerSpec) -> Result<String> {
        RealHostApi::udp_listen(self, app, spec).await
    }

    async fn udp_send(
        &self,
        app: &str,
        stream_id: &str,
        datagram: &[u8],
        peer: Option<&str>,
    ) -> Result<()> {
        RealHostApi::udp_send(self, app, stream_id, datagram, peer).await
    }

    async fn udp_close(&self, app: &str, stream_id: &str) -> Result<()> {
        RealHostApi::udp_close(self, app, stream_id).await
    }

    async fn raw_open(&self, app: &str, spec: &RawSpec) -> Result<String> {
        RealHostApi::raw_open(self, app, spec).await
    }

    async fn raw_send(
        &self,
        app: &str,
        stream_id: &str,
        packet: &[u8],
        destination: Option<&str>,
    ) -> Result<()> {
        RealHostApi::raw_send(self, app, stream_id, packet, destination).await
    }

    async fn raw_close(&self, app: &str, stream_id: &str) -> Result<()> {
        RealHostApi::raw_close(self, app, stream_id).await
    }

    async fn dns_serve(&self, app: &str, zone: &DnsZone) -> Result<String> {
        RealHostApi::dns_serve(self, app, zone).await
    }

    async fn dns_update(&self, app: &str, listener_id: &str, zone: &DnsZone) -> Result<()> {
        RealHostApi::dns_update(self, app, listener_id, zone).await
    }

    async fn dns_stop(&self, app: &str, listener_id: &str) -> Result<()> {
        RealHostApi::dns_stop(self, app, listener_id).await
    }

    // -- host programs ------------------------------------------------------

    async fn process_run(&self, app: &str, spec: &ProcessSpec) -> Result<ProcessOutput> {
        RealHostApi::process_run(self, app, spec).await
    }

    async fn process_spawn(&self, app: &str, spec: &ProcessSpec) -> Result<String> {
        RealHostApi::process_spawn(self, app, spec).await
    }

    async fn process_wait(&self, app: &str, process_id: &str) -> Result<ProcessOutput> {
        RealHostApi::process_wait(self, app, process_id).await
    }

    async fn process_kill(&self, app: &str, process_id: &str) -> Result<()> {
        RealHostApi::process_kill(self, app, process_id).await
    }

    // -- outbound http ------------------------------------------------------

    async fn http_request(&self, request: &OutboundHttpRequest) -> Result<OutboundHttpResponse> {
        RealHostApi::http_request(self, request).await
    }

    // -- config -------------------------------------------------------------

    async fn config_get(&self, plugin: &str, key: &str) -> Result<Option<String>> {
        RealHostApi::config_get(self, plugin, key).await
    }

    async fn config_set(&self, plugin: &str, key: &str, value: &str) -> Result<()> {
        RealHostApi::config_set(self, plugin, key, value).await
    }

    async fn config_list(&self, plugin: &str) -> Result<Vec<String>> {
        RealHostApi::config_list(self, plugin).await
    }

    async fn config_delete(&self, plugin: &str, key: &str) -> Result<()> {
        RealHostApi::config_delete(self, plugin, key).await
    }

    // -- schema -------------------------------------------------------------

    async fn schema_get_doctype(&self, doctype: &str) -> Result<SchemaDoctypeMeta> {
        RealHostApi::schema_get_doctype(self, doctype).await
    }

    async fn schema_list_doctypes(&self) -> Result<Vec<String>> {
        RealHostApi::schema_list_doctypes(self).await
    }

    // -- pki ----------------------------------------------------------------

    async fn pki_sign_csr(
        &self,
        plugin: &str,
        csr_der_b64: &str,
        ttl_seconds: u32,
    ) -> Result<String> {
        RealHostApi::pki_sign_csr(self, plugin, csr_der_b64, ttl_seconds).await
    }

    async fn pki_ca_cert(&self) -> Result<String> {
        RealHostApi::pki_ca_cert(self).await
    }

    async fn pki_revoke(&self, plugin: &str, fingerprint: &str) -> Result<()> {
        RealHostApi::pki_revoke(self, plugin, fingerprint).await
    }

    async fn pki_is_revoked(&self, fingerprint: &str) -> Result<bool> {
        RealHostApi::pki_is_revoked(self, fingerprint).await
    }

    // -- identity attachment ------------------------------------------------

    fn set_current_user(&self, user: Option<ferris_auth::User>) {
        RealHostApi::set_current_user(self, user);
    }

    fn set_certificate_service(&self, active: bool) {
        RealHostApi::set_certificate_service(self, active);
    }

    fn register_stream(&self, stream_id: &str, info: StreamInfo) {
        RealHostApi::register_stream(self, stream_id, info);
    }

    fn unregister_stream(&self, stream_id: &str) {
        RealHostApi::unregister_stream(self, stream_id);
    }

    fn stream_received(&self, stream_id: &str, bytes: usize) {
        RealHostApi::stream_received(self, stream_id, bytes);
    }

    fn stream_client_identity(&self, stream_id: &str) -> Option<ferris_core::ClientIdentity> {
        RealHostApi::stream_client_identity(self, stream_id)
    }
}
