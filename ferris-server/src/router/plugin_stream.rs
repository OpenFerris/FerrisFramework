//! Raw plugin stream endpoints (`handle-stream`): WebSocket and REST.

use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    extract::{
        Extension, Path as AxumPath, Request, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use ferris_auth::User;
use ferris_core::ClientIdentity;
use ferris_wasm::{PluginStore, StreamInfo};
use uuid::Uuid;

use super::plugin_http::PluginUser;
use super::plugins::{PLUGIN_BODY_LIMIT_BYTES, run_plugin};
use crate::client_identity::PluginClientIdentity;

/// Raw WebSocket endpoint exercising `handle-stream` (dev UX): every binary or
/// text frame is forwarded to the plugin and its reply is relayed back on the
/// socket. Closing the connection sends the end-of-stream (empty chunk)
/// signal.
///
/// Mounted only when `[server] dev` is on. In production the path does not
/// exist (404): this surface forwards frames with the fixed `plugins` route id
/// and an optional session, which is not the per-route auth an app declares.
pub(super) async fn plugin_stream_ws(
    State(plugins): State<Arc<PluginStore>>,
    AxumPath(name): AxumPath<String>,
    Extension(PluginUser(user)): Extension<PluginUser>,
    Extension(PluginClientIdentity(client_identity)): Extension<PluginClientIdentity>,
    ws: WebSocketUpgrade,
) -> Response {
    if plugins.get(&name).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let auth_required = plugin_stream_auth_required();
    if stream_edge_rejected(auth_required, &user, &client_identity) {
        return (
            StatusCode::UNAUTHORIZED,
            "authentication required for plugin streams",
        )
            .into_response();
    }
    ws.on_upgrade(move |socket| {
        run_plugin_stream(plugins, name, socket, user, client_identity, auth_required)
    })
}

/// Whether plugin streams must carry an authenticated session at the edge.
///
/// `FERRIS_PLUGIN_STREAM_AUTH=required` closes the anonymous-frame vector R-2
/// flags: an unauthenticated upgrade/frame is rejected with 401 before the
/// guest ever sees it. Defaults to `optional` so the local capture endpoints
/// stay usable without a session. The resolved flag is exposed per stream
/// through `streams::info` (`auth-required`).
fn plugin_stream_auth_required() -> bool {
    std::env::var("FERRIS_PLUGIN_STREAM_AUTH")
        .map(|v| v.eq_ignore_ascii_case("required"))
        .unwrap_or(false)
}

/// Whether the edge must refuse this stream request.
///
/// A verified client certificate counts as an authenticated caller: an mTLS
/// device carries no session or bearer token, so a `required` policy that only
/// looked at `user` locked every device out with 401 (H6 / plan Fase 2). The
/// certificate is bound by `bind_client_identity`; the plugin remains the
/// authority on what the certificate *means* (the host only forwards the facts).
fn stream_edge_rejected(
    auth_required: bool,
    user: &Option<User>,
    identity: &Option<ClientIdentity>,
) -> bool {
    auth_required && user.is_none() && identity.is_none()
}

async fn run_plugin_stream(
    plugins: Arc<PluginStore>,
    name: String,
    mut socket: WebSocket,
    user: Option<User>,
    client_identity: Option<ClientIdentity>,
    auth_required: bool,
) {
    let stream_id = format!("ws-{}", Uuid::new_v4());

    // Announce the transport so the plugin's `streams::info` import can answer
    // for this `stream-id` for the whole session.
    let _ = plugins.register_stream(
        &name,
        &stream_id,
        StreamInfo::new("websocket")
            .auth_required(auth_required)
            .client_identity(client_identity),
    );

    let mut shutdown = ferris_api::shutdown::subscribe();
    loop {
        if *shutdown.borrow() {
            break;
        }
        let frame = tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
                continue;
            }
            frame = socket.recv() => frame,
        };
        let Some(frame) = frame else { break };
        let Ok(frame) = frame else { break };
        let chunk = match frame {
            Message::Text(text) => text.as_str().as_bytes().to_vec(),
            Message::Binary(bytes) => bytes.to_vec(),
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue,
        };

        // The wasm call is blocking; park this worker out of the scheduler
        // while it runs (same pattern as the HTTP plugin handlers).
        let reply = run_plugin(|| {
            plugins
                .handle_stream_as(&name, &stream_id, &chunk, user.clone())
                .unwrap_or_default()
        })
        .await;
        if socket.send(Message::Binary(reply.into())).await.is_err() {
            break;
        }
    }

    // End of stream: forward the empty chunk (EOF) and forget the id.
    let _ = run_plugin(|| plugins.handle_stream_as(&name, &stream_id, &[], user.clone())).await;
    let _ = plugins.unregister_stream(&name, &stream_id);
}

/// One frame of a plugin stream over plain HTTP (dev UX). The request body is
/// forwarded to `handle-stream`; the reply comes back `application/octet-stream`.
///
/// Dev-only, like [`plugin_stream_ws`]: production does not mount
/// `/{name}/stream/*`.
pub(super) async fn plugin_stream_rest(
    State(plugins): State<Arc<PluginStore>>,
    AxumPath((name, stream_id)): AxumPath<(String, String)>,
    req: Request<Body>,
) -> Result<Response, StatusCode> {
    let user = req
        .extensions()
        .get::<PluginUser>()
        .and_then(|pu| pu.0.clone());
    let client_identity = req
        .extensions()
        .get::<PluginClientIdentity>()
        .and_then(|pi| pi.0.clone());
    let auth_required = plugin_stream_auth_required();
    if stream_edge_rejected(auth_required, &user, &client_identity) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let body = to_bytes(req.into_body(), PLUGIN_BODY_LIMIT_BYTES)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;

    // A stream may start on its first frame without a prior registration; make
    // `streams::info` answer for the id nonetheless.
    let _ = plugins.register_stream(
        &name,
        &stream_id,
        StreamInfo::new("rest")
            .auth_required(auth_required)
            .client_identity(client_identity),
    );

    let resp = run_plugin(move || plugins.handle_stream_as(&name, &stream_id, &body, user)).await;
    match resp {
        Ok(out) => axum::response::Response::builder()
            .header("content-type", "application/octet-stream")
            .body(Body::from(out))
            .map_err(|error| {
                tracing::error!(%error, "building the plugin stream response failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }),
        Err(_) => Err(StatusCode::BAD_REQUEST),
    }
}

/// Close a plugin stream: forwards the end-of-stream (empty chunk) signal and
/// releases the `stream-id`.
pub(super) async fn plugin_stream_eof(
    State(plugins): State<Arc<PluginStore>>,
    AxumPath((name, stream_id)): AxumPath<(String, String)>,
    req: Request<Body>,
) -> StatusCode {
    let user = req
        .extensions()
        .get::<PluginUser>()
        .and_then(|pu| pu.0.clone());
    let client_identity = req
        .extensions()
        .get::<PluginClientIdentity>()
        .and_then(|pi| pi.0.clone());
    if stream_edge_rejected(plugin_stream_auth_required(), &user, &client_identity) {
        return StatusCode::UNAUTHORIZED;
    }
    let _ = run_plugin(|| plugins.handle_stream_as(&name, &stream_id, &[], user)).await;
    let _ = plugins.unregister_stream(&name, &stream_id);
    StatusCode::NO_CONTENT
}
