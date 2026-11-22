//! The rejecting default [`HostApi`] implementation.

use anyhow::Result;
use async_trait::async_trait;

use super::api::HostApi;
use super::error::BridgeError;
use super::types::{
    DnsZone, DocAggregateRow, DocAggregateSpec, DocOp, DocRecord, ListDocsQuery,
    OutboundHttpRequest, OutboundHttpResponse, ProcessOutput, ProcessSpec, RawSpec, ScheduleInfo,
    SchemaDoctypeMeta, StreamInfo, UdpListenerSpec, UdpSpec, UserContext,
};

/// A host service that rejects every call.
///
/// Useful as the default until a real backend ([`HostApi`] implementation) is
/// wired in, and as the safe fallback for plugins whose world requires services
/// the host did not enable.
pub struct NullHost;

#[async_trait]
impl HostApi for NullHost {
    async fn docstore_list(
        &self,
        _doctype: &str,
        _query: &ListDocsQuery,
    ) -> Result<Vec<DocRecord>> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_get(&self, _doctype: &str, _name: &str) -> Result<DocRecord> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_get_many(&self, _doctype: &str, _names: &[String]) -> Result<Vec<DocRecord>> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_create(&self, _doctype: &str, _data: &str) -> Result<DocRecord> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_upsert(&self, _doctype: &str, _data: &str) -> Result<DocRecord> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_write_no_events(&self, _doctype: &str, _data: &str) -> Result<DocRecord> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_update(&self, _doctype: &str, _name: &str, _data: &str) -> Result<DocRecord> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_delete(&self, _doctype: &str, _name: &str) -> Result<()> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_count(&self, _doctype: &str, _query: &ListDocsQuery) -> Result<u64> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_aggregate(
        &self,
        _doctype: &str,
        _query: &ListDocsQuery,
        _group_by: &[String],
        _aggregates: &[DocAggregateSpec],
    ) -> Result<Vec<DocAggregateRow>> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_bulk_create(
        &self,
        _doctype: &str,
        _records: &[String],
    ) -> Result<Vec<DocRecord>> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_apply(&self, _ops: &[DocOp]) -> Result<Vec<DocRecord>> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_rename(
        &self,
        _doctype: &str,
        _name: &str,
        _new_name: &str,
    ) -> Result<DocRecord> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_set_docstatus(
        &self,
        _doctype: &str,
        _name: &str,
        _action: &str,
    ) -> Result<DocRecord> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn docstore_doctype_json(&self, _doctype: &str) -> Result<String> {
        anyhow::bail!("docstore service is not enabled in this runtime")
    }
    async fn field_types(&self) -> Vec<String> {
        Vec::new()
    }
    async fn pubsub_publish(
        &self,
        _channel: &str,
        _event_type: &str,
        _payload: &str,
    ) -> Result<()> {
        anyhow::bail!("pubsub service is not enabled in this runtime")
    }
    async fn notification_send(
        &self,
        _to: &str,
        _channel: &str,
        _title: &str,
        _message: &str,
    ) -> Result<()> {
        anyhow::bail!("notification service is not enabled in this runtime")
    }
    async fn naming_next(&self, _series: &str) -> Result<String> {
        anyhow::bail!("naming service is not enabled in this runtime")
    }
    async fn session_current(&self) -> Result<UserContext> {
        anyhow::bail!("session service is not enabled in this runtime")
    }
    async fn session_can(
        &self,
        _doctype: &str,
        _action: &str,
        _name: Option<&str>,
    ) -> Result<bool> {
        anyhow::bail!("session service is not enabled in this runtime")
    }
    async fn session_can_many(
        &self,
        _checks: &[(String, String, Option<String>)],
    ) -> Result<Vec<bool>> {
        anyhow::bail!("session service is not enabled in this runtime")
    }
    async fn scheduler_schedule(&self, _plugin: &str, _record: &str) -> Result<String> {
        anyhow::bail!("scheduler service is not enabled in this runtime")
    }
    async fn scheduler_enqueue(
        &self,
        _plugin: &str,
        _job_type: &str,
        _payload: &str,
    ) -> Result<()> {
        anyhow::bail!("scheduler service is not enabled in this runtime")
    }
    async fn scheduler_cancel(&self, _plugin: &str, _schedule_id: &str) -> Result<()> {
        anyhow::bail!("scheduler service is not enabled in this runtime")
    }
    async fn scheduler_list(&self, _plugin: &str) -> Result<Vec<ScheduleInfo>> {
        anyhow::bail!("scheduler service is not enabled in this runtime")
    }
    async fn streams_info(&self, _stream_id: &str) -> Result<StreamInfo> {
        anyhow::bail!("streams are not tracked by this runtime")
    }
    async fn streams_state(&self, _stream_id: &str) -> Result<Vec<u8>> {
        anyhow::bail!("streams are not tracked by this runtime")
    }
    async fn streams_set_state(&self, _stream_id: &str, _pending: &[u8]) -> Result<()> {
        anyhow::bail!("streams are not tracked by this runtime")
    }
    // Datagram sockets, DNS and host programs have no implementation here by
    // construction: a null host owns no descriptor and no child process. The
    // typed `unsupported` failure is what `platform::features` promises a guest
    // whose host cannot honour the capability.
    async fn udp_connect(&self, _app: &str, _spec: &UdpSpec) -> Result<String> {
        Err(BridgeError::unsupported(
            "io::udp-connect (the null host owns no socket)",
        ))
    }
    async fn udp_listen(&self, _app: &str, _spec: &UdpListenerSpec) -> Result<String> {
        Err(BridgeError::unsupported(
            "io::udp-listen (the null host owns no socket)",
        ))
    }
    async fn udp_send(
        &self,
        _app: &str,
        _stream_id: &str,
        _datagram: &[u8],
        _peer: Option<&str>,
    ) -> Result<()> {
        Err(BridgeError::unsupported(
            "io::udp-send (the null host owns no socket)",
        ))
    }
    async fn udp_close(&self, _app: &str, _stream_id: &str) -> Result<()> {
        Err(BridgeError::unsupported(
            "io::udp-close (the null host owns no socket)",
        ))
    }
    async fn raw_open(&self, _app: &str, _spec: &RawSpec) -> Result<String> {
        Err(BridgeError::unsupported(
            "io::raw-open (the null host owns no socket)",
        ))
    }
    async fn raw_send(
        &self,
        _app: &str,
        _stream_id: &str,
        _packet: &[u8],
        _destination: Option<&str>,
    ) -> Result<()> {
        Err(BridgeError::unsupported(
            "io::raw-send (the null host owns no socket)",
        ))
    }
    async fn raw_close(&self, _app: &str, _stream_id: &str) -> Result<()> {
        Err(BridgeError::unsupported(
            "io::raw-close (the null host owns no socket)",
        ))
    }
    async fn dns_serve(&self, _app: &str, _zone: &DnsZone) -> Result<String> {
        Err(BridgeError::unsupported(
            "io::dns-serve (the null host serves no DNS)",
        ))
    }
    async fn dns_update(&self, _app: &str, _listener_id: &str, _zone: &DnsZone) -> Result<()> {
        Err(BridgeError::unsupported(
            "io::dns-update (the null host serves no DNS)",
        ))
    }
    async fn dns_stop(&self, _app: &str, _listener_id: &str) -> Result<()> {
        Err(BridgeError::unsupported(
            "io::dns-stop (the null host serves no DNS)",
        ))
    }
    async fn process_run(&self, _app: &str, _spec: &ProcessSpec) -> Result<ProcessOutput> {
        Err(BridgeError::unsupported(
            "platform::process-run (the null host runs no program)",
        ))
    }
    async fn process_spawn(&self, _app: &str, _spec: &ProcessSpec) -> Result<String> {
        Err(BridgeError::unsupported(
            "platform::process-spawn (the null host runs no program)",
        ))
    }
    async fn process_wait(&self, _app: &str, _process_id: &str) -> Result<ProcessOutput> {
        Err(BridgeError::unsupported(
            "platform::process-wait (the null host runs no program)",
        ))
    }
    async fn process_kill(&self, _app: &str, _process_id: &str) -> Result<()> {
        Err(BridgeError::unsupported(
            "platform::process-kill (the null host runs no program)",
        ))
    }
    async fn http_request(&self, _request: &OutboundHttpRequest) -> Result<OutboundHttpResponse> {
        anyhow::bail!("http service is not enabled in this runtime")
    }
    async fn config_get(&self, _plugin: &str, _key: &str) -> Result<Option<String>> {
        anyhow::bail!("config service is not enabled in this runtime")
    }
    async fn config_set(&self, _plugin: &str, _key: &str, _value: &str) -> Result<()> {
        anyhow::bail!("config service is not enabled in this runtime")
    }
    async fn config_list(&self, _plugin: &str) -> Result<Vec<String>> {
        anyhow::bail!("config service is not enabled in this runtime")
    }
    async fn config_delete(&self, _plugin: &str, _key: &str) -> Result<()> {
        anyhow::bail!("config service is not enabled in this runtime")
    }
    async fn schema_get_doctype(&self, _doctype: &str) -> Result<SchemaDoctypeMeta> {
        anyhow::bail!("schema service is not enabled in this runtime")
    }
    async fn schema_list_doctypes(&self) -> Result<Vec<String>> {
        anyhow::bail!("schema service is not enabled in this runtime")
    }
    async fn pki_sign_csr(
        &self,
        _plugin: &str,
        _csr_der_b64: &str,
        _ttl_seconds: u32,
    ) -> Result<String> {
        anyhow::bail!("pki service is not enabled in this runtime")
    }
    async fn pki_ca_cert(&self) -> Result<String> {
        anyhow::bail!("pki service is not enabled in this runtime")
    }
    async fn pki_revoke(&self, _plugin: &str, _fingerprint: &str) -> Result<()> {
        anyhow::bail!("pki service is not enabled in this runtime")
    }
    async fn pki_is_revoked(&self, _fingerprint: &str) -> Result<bool> {
        anyhow::bail!("pki service is not enabled in this runtime")
    }
}
