//! The receive loop shared by UDP and raw sockets.

use std::net::SocketAddr;
use std::sync::Arc;

use ferris_wasm::PluginStore;
use tokio::net::UdpSocket;
use tokio::task::AbortHandle;

use crate::plugin_net::run_guest;

/// Pump one UDP or raw socket: receive, call `handle-datagram`, write the reply.
/// Pump one UDP or raw socket: receive, call `handle-datagram`, write the reply.
pub(super) fn spawn_udp_pump(
    store: Arc<PluginStore>,
    app: String,
    id: String,
    socket: Arc<UdpSocket>,
    connected: bool,
    max: usize,
) -> AbortHandle {
    let task = tokio::spawn(async move {
        let mut buffer = vec![0_u8; max];
        loop {
            let received = if connected {
                socket.recv(&mut buffer).await.map(|n| (n, None))
            } else {
                socket
                    .recv_from(&mut buffer)
                    .await
                    .map(|(n, peer)| (n, Some(peer)))
            };
            let (size, peer) = match received {
                Ok(value) => value,
                Err(error) => {
                    tracing::debug!(app = %app, stream = %id, error = %error, "datagram socket stopped");
                    break;
                }
            };
            let peer = peer.map_or_else(
                || {
                    socket
                        .peer_addr()
                        .map(|addr| addr.to_string())
                        .unwrap_or_default()
                },
                |addr| addr.to_string(),
            );
            let data = buffer[..size].to_vec();
            let call = {
                let store = Arc::clone(&store);
                let app = app.clone();
                let id = id.clone();
                let peer_for_call = peer.clone();
                run_guest(move || {
                    store
                        .handle_datagram(&app, &id, &data, &peer_for_call)
                        .map_err(|e| anyhow::anyhow!("{e}"))
                })
                .await
            };
            let reply = match call {
                Ok(reply) => reply,
                Err(error) => {
                    tracing::warn!(app = %app, stream = %id, error = %error, "handle-datagram failed");
                    continue;
                }
            };
            if reply.is_empty() {
                continue;
            }
            if reply.len() > max {
                tracing::warn!(
                    app = %app, stream = %id, size = reply.len(), cap = max,
                    "handle-datagram reply exceeds the socket cap and was dropped"
                );
                continue;
            }
            let sent = match &peer.parse::<SocketAddr>() {
                Ok(destination) => socket.send_to(&reply, *destination).await.map(|_| ()),
                Err(_) => socket.send(&reply).await.map(|_| ()),
            };
            if let Err(error) = sent {
                tracing::debug!(app = %app, stream = %id, error = %error, "datagram reply failed");
            }
        }
    });
    task.abort_handle()
}
