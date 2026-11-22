//! The declared `%stream` route that is an HTTP upgrade.
//!
//! A WebSocket listener is not a socket the host binds: the guest declares
//! `route { kind: %stream, path, listener: { transport: websocket, … } }` and the
//! host mounts a `WebSocketUpgrade` at that path, exactly like a declared HTTP
//! route is mounted at the root. Three things make that safe:
//!
//! * **Auth before the upgrade.** The route's `auth` (`none`/`session`/
//!   capability) is enforced by the same [`authorize`] the request path uses, so
//!   an anonymous caller gets `401` instead of an open stream. The capability
//!   header is not a frame: the guest never sees it.
//! * **The stream identity reaches the guest.** The connection is registered as
//!   a `stream-id` (protocol `websocket`, peer facts, mTLS identity) and the
//!   guest is told `opened` before the first frame, so `streams::info` answers
//!   for the whole session.
//! * **A clean close.** Whatever ends the loop — the peer's Close frame, a guest
//!   error, a process shutdown — the host sends the Close frame back and reports
//!   `closed` (or `error`) exactly once, then forgets the id.
//!
//! The reply to a frame is the frame's return value: the guest returns
//! `list<u8>`, and empty means "ack/drop" (nothing goes on the wire), so a guest
//! that has nothing to say does not send an empty frame the peer would have to
//! interpret.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{
        Extension, Request, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use ferris_auth::User;
use ferris_core::ClientIdentity;
use ferris_wasm::{PluginStore, RouteAuth, StreamEvent, StreamInfo};
use futures_util::SinkExt;
use tracing::warn;
use uuid::Uuid;

use super::mount::{CAPABILITY_HEADER, DeclaredRouteState, RouteContext, StreamLimits, authorize};
use crate::client_identity::PluginClientIdentity;
use crate::router::plugin_http::PluginUser;
use crate::router::plugins::run_plugin;

/// Serve one declared `%stream` route: enforce its auth, then upgrade.
pub(super) async fn declared_stream_handler(
    State(state): State<DeclaredRouteState>,
    Extension(context): Extension<RouteContext>,
    Extension(PluginUser(session_user)): Extension<PluginUser>,
    Extension(PluginClientIdentity(client_identity)): Extension<PluginClientIdentity>,
    ws: WebSocketUpgrade,
    req: Request<Body>,
) -> Response {
    let Some(plugins) = state.plugins.clone() else {
        // Mounted without a store is a wiring bug, and a 404 is the honest
        // answer: nothing can serve this path right now.
        warn!(route = %context.route_id, "declared stream route has no plugin store");
        return StatusCode::NOT_FOUND.into_response();
    };

    let supplied = req
        .headers()
        .get(CAPABILITY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let user = match authorize(&plugins, &context, session_user, supplied.as_deref()).await {
        Ok(user) => user,
        // The gate answers before the upgrade: the client sees a refused
        // handshake, never an open stream.
        Err(status) => return status.into_response(),
    };
    // `stream-info.auth-required` reports what the declaration enforced, so the
    // guest can tell a gated stream from a public one.
    let auth_required = !matches!(context.auth, RouteAuth::None);
    let limits = context.stream.unwrap_or(StreamLimits {
        max_frame: None,
        idle: None,
    });
    let app = context.app;
    let route_id = context.route_id;
    ws.on_upgrade(move |socket| {
        run_declared_stream(
            plugins,
            app,
            route_id,
            limits,
            socket,
            user,
            client_identity,
            auth_required,
        )
    })
}

/// Drive one upgraded socket: register the stream, forward frames, close it.
#[allow(clippy::too_many_arguments)] // the connection's identity, policy and owner
async fn run_declared_stream(
    plugins: Arc<PluginStore>,
    app: String,
    route_id: String,
    limits: StreamLimits,
    mut socket: WebSocket,
    user: Option<User>,
    identity: Option<ClientIdentity>,
    auth_required: bool,
) {
    let stream_id = format!("ws-{}", Uuid::new_v4());
    let info = StreamInfo::new("websocket")
        .auth_required(auth_required)
        .client_identity(identity);
    if let Err(e) = plugins.register_stream(&app, &stream_id, info) {
        warn!(app = %app, route = %route_id, error = %e, "stream registration failed");
    }
    if let Err(e) = deliver(
        &plugins,
        &app,
        &stream_id,
        StreamEvent::Opened,
        user.clone(),
    )
    .await
    {
        warn!(app = %app, stream = %stream_id, error = %e, "opened event failed");
    }

    let mut failure: Option<String> = None;
    let mut shutdown = ferris_api::shutdown::subscribe();
    loop {
        if *shutdown.borrow() {
            break;
        }
        let idle = limits.idle;
        let frame = tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
                continue;
            }
            frame = socket.recv() => frame,
            // The declared idle timeout, if any. Without one the branch parks
            // forever and never wins the select.
            () = async move {
                match idle {
                    Some(idle) => tokio::time::sleep(idle).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                failure = Some(format!(
                    "idle for {}s",
                    limits.idle.map_or(0, |idle| idle.as_secs())
                ));
                break;
            }
        };
        let Some(frame) = frame else { break };
        let chunk = match frame {
            Ok(Message::Text(text)) => text.as_str().as_bytes().to_vec(),
            Ok(Message::Binary(bytes)) => bytes.to_vec(),
            Ok(Message::Close(_)) => break,
            // Ping/Pong belong to the protocol (tungstenite answers a Ping
            // itself), not to the guest's frames.
            Ok(Message::Ping(_) | Message::Pong(_)) => continue,
            Err(e) => {
                failure = Some(format!("reading a websocket frame: {e}"));
                break;
            }
        };
        if let Some(max) = limits.max_frame.filter(|max| chunk.len() > *max) {
            // A declared cap is enforcement: the frame is refused loudly, never
            // silently truncated.
            failure = Some(format!(
                "frame of {} bytes exceeds the declared {max} byte cap",
                chunk.len()
            ));
            break;
        }

        // The wasm call is blocking; park this worker out of the scheduler while
        // it runs (the same pattern the `/plugins/*` stream handler uses).
        let reply =
            run_plugin(|| plugins.handle_stream_as(&app, &stream_id, &chunk, user.clone())).await;
        match reply {
            Ok(reply) if !reply.is_empty() => {
                if let Err(e) = socket.send(Message::Binary(reply.into())).await {
                    failure = Some(format!("writing a websocket frame: {e}"));
                    break;
                }
            }
            // Empty is ack/drop: the guest consumed the frame and has nothing to
            // relay back.
            Ok(_) => {}
            Err(e) => {
                failure = Some(format!("handle-stream failed: {e}"));
                break;
            }
        }
    }

    // A clean close reaches the peer whether the guest finished, the process is
    // shutting down or the loop failed: tungstenite flushes the Close frame.
    let _ = socket.close().await;
    let terminal = failure.map_or(StreamEvent::Closed, StreamEvent::Error);
    if let Err(e) = deliver(&plugins, &app, &stream_id, terminal, user).await {
        warn!(app = %app, stream = %stream_id, error = %e, "terminal stream event failed");
    }
    let _ = plugins.unregister_stream(&app, &stream_id);
}

/// Deliver one lifecycle event as the caller the stream was opened for.
async fn deliver(
    plugins: &Arc<PluginStore>,
    app: &str,
    stream_id: &str,
    event: StreamEvent,
    user: Option<User>,
) -> anyhow::Result<()> {
    let plugins = Arc::clone(plugins);
    let app = app.to_string();
    let id = stream_id.to_string();
    run_plugin(move || {
        plugins
            .handle_stream_event_as(&app, &id, event, user)
            .map_err(|e| anyhow::anyhow!("{e}"))
    })
    .await
}
