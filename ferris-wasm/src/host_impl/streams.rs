//! `streams` interface of [`HostApi`](crate::host::HostApi) and the stream
//! registry the host transport feeds.

use super::RealHostApi;
use crate::host::{
    DnsZone, OutboundStreamSpec, RawSpec, StreamInfo, StreamListenerSpec, UdpListenerSpec, UdpSpec,
    new_registration_id, stream_service, unix_millis,
};
use anyhow::Result;

impl RealHostApi {
    // -- streams ------------------------------------------------------------

    /// The sockets themselves belong to the running server: this forwards to the
    /// process-wide [`StreamService`](crate::host::StreamService) and, when
    /// none is registered, reports the import as unimplemented instead of
    /// fabricating a connection.
    pub(super) async fn streams_connect(
        &self,
        app: &str,
        spec: &OutboundStreamSpec,
    ) -> Result<String> {
        match stream_service() {
            Some(transport) => transport.connect(app, spec).await,
            None => Err(crate::host::BridgeError::unimplemented(
                "io::connect-stream (no stream transport registered in this process)",
            )),
        }
    }

    pub(super) async fn streams_listen(
        &self,
        app: &str,
        spec: &StreamListenerSpec,
    ) -> Result<String> {
        match stream_service() {
            Some(transport) => transport.listen(app, spec).await,
            None => Err(crate::host::BridgeError::unimplemented(
                "io::listen-stream (no stream transport registered in this process)",
            )),
        }
    }

    pub(super) async fn streams_send(
        &self,
        app: &str,
        stream_id: &str,
        bytes: &[u8],
    ) -> Result<()> {
        match stream_service() {
            Some(transport) => transport.send(app, stream_id, bytes).await,
            None => Err(crate::host::BridgeError::unimplemented(
                "io::send-stream (no stream transport registered in this process)",
            )),
        }
    }

    pub(super) async fn streams_close(&self, app: &str, stream_id: &str) -> Result<()> {
        match stream_service() {
            Some(transport) => transport.close(app, stream_id).await,
            None => Err(crate::host::BridgeError::unimplemented(
                "io::close-stream (no stream transport registered in this process)",
            )),
        }
    }

    // -- datagram sockets ---------------------------------------------------

    /// The datagram descriptor belongs to the running server for the same
    /// reason the byte streams do: this runtime has no tokio reactor of its own
    /// and no socket budget. Forward to the process-wide transport and answer
    /// `unsupported` when none is registered, so the guest learns the surface
    /// is absent instead of believing a socket was opened.
    pub(super) async fn udp_connect(&self, app: &str, spec: &UdpSpec) -> Result<String> {
        match stream_service() {
            Some(transport) => transport.udp_connect(app, spec).await,
            None => Err(crate::host::BridgeError::unsupported(
                "io::udp-connect (no datagram transport registered in this process)",
            )),
        }
    }

    pub(super) async fn udp_listen(&self, app: &str, spec: &UdpListenerSpec) -> Result<String> {
        match stream_service() {
            Some(transport) => transport.udp_listen(app, spec).await,
            None => Err(crate::host::BridgeError::unsupported(
                "io::udp-listen (no datagram transport registered in this process)",
            )),
        }
    }

    pub(super) async fn udp_send(
        &self,
        app: &str,
        stream_id: &str,
        datagram: &[u8],
        peer: Option<&str>,
    ) -> Result<()> {
        match stream_service() {
            Some(transport) => transport.udp_send(app, stream_id, datagram, peer).await,
            None => Err(crate::host::BridgeError::unsupported(
                "io::udp-send (no datagram transport registered in this process)",
            )),
        }
    }

    pub(super) async fn udp_close(&self, app: &str, stream_id: &str) -> Result<()> {
        match stream_service() {
            Some(transport) => transport.udp_close(app, stream_id).await,
            None => Err(crate::host::BridgeError::unsupported(
                "io::udp-close (no datagram transport registered in this process)",
            )),
        }
    }

    pub(super) async fn raw_open(&self, app: &str, spec: &RawSpec) -> Result<String> {
        match stream_service() {
            Some(transport) => transport.raw_open(app, spec).await,
            None => Err(crate::host::BridgeError::unsupported(
                "io::raw-open (no raw-socket transport registered in this process)",
            )),
        }
    }

    pub(super) async fn raw_send(
        &self,
        app: &str,
        stream_id: &str,
        packet: &[u8],
        destination: Option<&str>,
    ) -> Result<()> {
        match stream_service() {
            Some(transport) => {
                transport
                    .raw_send(app, stream_id, packet, destination)
                    .await
            }
            None => Err(crate::host::BridgeError::unsupported(
                "io::raw-send (no raw-socket transport registered in this process)",
            )),
        }
    }

    pub(super) async fn raw_close(&self, app: &str, stream_id: &str) -> Result<()> {
        match stream_service() {
            Some(transport) => transport.raw_close(app, stream_id).await,
            None => Err(crate::host::BridgeError::unsupported(
                "io::raw-close (no raw-socket transport registered in this process)",
            )),
        }
    }

    pub(super) async fn dns_serve(&self, app: &str, zone: &DnsZone) -> Result<String> {
        match stream_service() {
            Some(transport) => transport.dns_serve(app, zone).await,
            None => Err(crate::host::BridgeError::unsupported(
                "io::dns-serve (no DNS transport registered in this process)",
            )),
        }
    }

    pub(super) async fn dns_update(
        &self,
        app: &str,
        listener_id: &str,
        zone: &DnsZone,
    ) -> Result<()> {
        match stream_service() {
            Some(transport) => transport.dns_update(app, listener_id, zone).await,
            None => Err(crate::host::BridgeError::unsupported(
                "io::dns-update (no DNS transport registered in this process)",
            )),
        }
    }

    pub(super) async fn dns_stop(&self, app: &str, listener_id: &str) -> Result<()> {
        match stream_service() {
            Some(transport) => transport.dns_stop(app, listener_id).await,
            None => Err(crate::host::BridgeError::unsupported(
                "io::dns-stop (no DNS transport registered in this process)",
            )),
        }
    }

    pub(super) async fn streams_info(&self, stream_id: &str) -> Result<StreamInfo> {
        self.streams
            .lock()
            .get(stream_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown or closed stream `{stream_id}`"))
    }

    pub(super) async fn streams_state(&self, stream_id: &str) -> Result<Vec<u8>> {
        if !self.streams.lock().contains_key(stream_id) {
            anyhow::bail!("unknown or closed stream `{stream_id}`");
        }
        Ok(self
            .stream_buffers
            .lock()
            .get(stream_id)
            .cloned()
            .unwrap_or_default())
    }

    pub(super) async fn streams_set_state(&self, stream_id: &str, pending: &[u8]) -> Result<()> {
        if !self.streams.lock().contains_key(stream_id) {
            anyhow::bail!("unknown or closed stream `{stream_id}`");
        }
        let mut buffers = self.stream_buffers.lock();
        if pending.is_empty() {
            buffers.remove(stream_id);
        } else {
            buffers.insert(stream_id.to_string(), pending.to_vec());
        }
        Ok(())
    }

    pub(super) fn register_stream(&self, stream_id: &str, mut info: StreamInfo) {
        // Idempotent: the REST transport re-registers the same `stream-id` on
        // every POST, so this must **not** reset the carry-over buffer (that
        // would defeat R-1 for HTTP-framed streams) nor the stream's age
        // (`created-at`) and byte count, which a re-registration used to wipe
        // with a fresh `StreamInfo`. Buffers are dropped on
        // `unregister_stream`, and WS ids are UUIDs that never collide.
        //
        // The identity, however, belongs to the **connection that sent this
        // request**, not to the stream-id: `bind_client_identity` fills it from
        // the verified peer certificate of *this* connection (or `None` when the
        // client presented none). So the incoming value is authoritative and is
        // never inherited from an earlier registration — inheriting let a later
        // client with no certificate (or a different one) reuse the id and be
        // treated as the first device (finding H4).
        //
        // A changed identity is a **recreation** of the same id by another
        // connection: the age is re-stamped so `created-at` changes and a guest
        // can detect the replacement instead of trusting what it cached
        // (mTLS plan Fase 4 / H4). The `registration-id` is re-stamped for the
        // same reason — the explicit recreation signal, independent of the
        // `created-at` a REST re-registration of the *same* stream keeps. The
        // byte count is kept so the per-stream budget stays conservative
        // (never looser than before).
        let mut streams = self.streams.lock();
        if let Some(existing) = streams.get(stream_id) {
            info.bytes_received = existing.bytes_received;
            if existing.client_identity == info.client_identity {
                info.created_at = existing.created_at;
                info.registration_id.clone_from(&existing.registration_id);
            } else {
                info.created_at = unix_millis();
                info.registration_id = new_registration_id();
            }
        }
        streams.insert(stream_id.to_string(), info);
    }

    pub(super) fn unregister_stream(&self, stream_id: &str) {
        self.streams.lock().remove(stream_id);
        self.stream_buffers.lock().remove(stream_id);
    }

    pub(super) fn stream_received(&self, stream_id: &str, bytes: usize) {
        if let Some(info) = self.streams.lock().get_mut(stream_id) {
            info.bytes_received = info.bytes_received.saturating_add(bytes as u64);
        }
    }

    pub(super) fn stream_client_identity(
        &self,
        stream_id: &str,
    ) -> Option<ferris_core::ClientIdentity> {
        self.streams
            .lock()
            .get(stream_id)
            .and_then(|info| info.client_identity.clone())
    }
}
