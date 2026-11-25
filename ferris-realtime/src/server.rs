//! Realtime server - main entry point

use crate::{PresenceManager, RealtimeEvent, RealtimeHandler, RoomManager};
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::Utc;
use ferris_auth::AuthState;
use ferris_core::{Error, RealtimeError, Result};
use socketioxide::{SocketIo, layer::SocketIoLayer};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

/// Realtime server configuration
///
/// Every field here is consumed: `ping_interval_secs`, `ping_timeout_secs` and
/// `max_message_size` feed `socketioxide`'s engine configuration; `host`/`port`
/// bind the listener; `cors_origins` build the CORS layer. The old `workers`,
/// `max_connections_per_worker` and `enable_compression` fields were never read
/// (socketioxide 0.10 has no compression toggle and this server is
/// single-process), so they were removed rather than left lying (RT7).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RealtimeConfig {
    pub host: String,
    pub port: u16,
    pub ping_interval_secs: u64,
    pub ping_timeout_secs: u64,
    pub cors_origins: Vec<String>,
    pub max_message_size: usize,
}

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8001,
            ping_interval_secs: 25,
            ping_timeout_secs: 60,
            cors_origins: Vec::new(),
            max_message_size: 1024 * 1024, // 1MB
        }
    }
}

/// Realtime server state
pub struct RealtimeServer {
    config: RealtimeConfig,
    socket_io: SocketIo,
    socket_io_layer: SocketIoLayer,
    room_manager: Arc<RoomManager>,
    presence_manager: Arc<PresenceManager>,
    auth_state: Option<AuthState>,
    /// The same handler the Socket.IO namespace closure owns, kept here so
    /// `with_auth` and the notification helpers reach the live emitter
    /// (V3-RT-1/V3-RT-2).
    handler: RealtimeHandler,
}

impl RealtimeServer {
    pub fn new(config: RealtimeConfig) -> Self {
        // Wire the engine knobs the config actually documents. Before this the
        // builder was never touched, so ping/timeout/message-size were inert
        // (RT7).
        let (layer, io) = SocketIo::builder()
            .ping_interval(Duration::from_secs(config.ping_interval_secs))
            .ping_timeout(Duration::from_secs(config.ping_timeout_secs))
            .max_payload(config.max_message_size as u64)
            .build_layer();

        let room_manager = Arc::new(RoomManager::new());
        let presence_manager = Arc::new(PresenceManager::new());

        let handler = RealtimeHandler::new(room_manager.clone(), presence_manager.clone())
            .with_socket_io(io.clone());

        // Register handler with socket.io. `RealtimeHandler` is `Clone` and its
        // mutable bits (`auth_state`, `connections`) are behind `Arc`, so the
        // clone moved into the closure and the clone stored on `Self` share the
        // same state: auth attached later through `with_auth` is visible to
        // every already-registered socket (V3-RT-1).
        let socket_handler = handler.clone();
        io.ns("/", move |socket: socketioxide::extract::SocketRef| {
            let handler = socket_handler.clone();
            async move {
                handler.on_connect(socket).await;
            }
        });

        Self {
            config,
            socket_io: io,
            socket_io_layer: layer,
            room_manager,
            presence_manager,
            auth_state: None,
            handler,
        }
    }

    pub fn with_auth(mut self, auth_state: AuthState) -> Self {
        // Attach to both the stats gate and the live socket handler; before
        // this only the former was set, so sockets stayed anonymous (V3-RT-1).
        self.handler.set_auth(auth_state.clone());
        self.auth_state = Some(auth_state);
        self
    }

    pub fn room_manager(&self) -> &Arc<RoomManager> {
        &self.room_manager
    }

    pub fn presence_manager(&self) -> &Arc<PresenceManager> {
        &self.presence_manager
    }

    /// The live handler, so the server wiring can install it as the
    /// process-wide document-notification sink (RT2).
    pub fn handler(&self) -> &RealtimeHandler {
        &self.handler
    }

    pub fn socket_io(&self) -> &SocketIo {
        &self.socket_io
    }

    /// The socket.io middleware layer, for mounting realtime on a shared router
    /// instead of running a dedicated listener.
    pub fn socket_io_layer(&self) -> SocketIoLayer {
        self.socket_io_layer.clone()
    }

    /// Start the periodic maintenance task: presence housekeeping plus a
    /// re-check of every live connection's authorization (RT-01/RT14).
    ///
    /// `start()` calls this for the standalone listener. A deployment that
    /// mounts [`RealtimeServer::socket_io_layer`] inside a shared router does
    /// not call `start()`, so the composition root
    /// (`ferris-server/src/router/build.rs`) must call this once too — the
    /// task is what turns a revoked session, a disabled account or a withdrawn
    /// role into a closed socket.
    ///
    /// Returns `false` when there is no Tokio runtime to spawn on.
    #[must_use]
    pub fn spawn_maintenance(&self, interval: Duration) -> bool {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return false;
        };
        let presence = self.presence_manager.clone();
        let handler = self.handler.clone();
        handle.spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                presence.update_statuses();
                presence.cleanup_stale(chrono::Duration::hours(1));
                let closed = handler.revalidate_connections().await;
                if closed > 0 {
                    tracing::info!(
                        closed,
                        "realtime: closed connections whose authorization was revoked"
                    );
                }
            }
        });
        true
    }

    /// Start the server
    pub async fn start(self) -> Result<()> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&addr).await.map_err(|e| {
            Error::Realtime(RealtimeError::Connection(format!(
                "Failed to bind to {}: {}",
                addr, e
            )))
        })?;

        tracing::info!("Realtime server listening on {}", addr);

        // Presence maintenance used to be unreachable: `update_statuses` and
        // `cleanup_stale` had no caller, so an abandoned presence record lived
        // for the process lifetime (RT14). The same task re-checks the
        // authorization of every live socket, so a revoked session or a
        // withdrawn role closes the connection instead of leaving it receiving
        // documents (RT-01).
        let _ = self.spawn_maintenance(Duration::from_secs(60));

        let app = self.create_router();

        axum::serve(listener, app).await.map_err(|e| {
            Error::Realtime(RealtimeError::Connection(format!("Server error: {}", e)))
        })?;

        Ok(())
    }

    fn create_router(self) -> Router {
        // A malformed configured origin used to be dropped silently, which
        // made a CORS typo look like "no origin" (RT11).
        let cors_origins: Vec<axum::http::HeaderValue> = self
            .config
            .cors_origins
            .iter()
            .filter_map(|raw| match raw.parse() {
                Ok(origin) => Some(origin),
                Err(e) => {
                    tracing::warn!("ignoring invalid CORS origin {raw:?}: {e}");
                    None
                }
            })
            .collect();

        let wildcard = self.config.cors_origins.iter().any(|o| o == "*");
        let mut cors = CorsLayer::new()
            .allow_origin(cors_origins)
            .allow_methods([
                axum::http::Method::GET,
                axum::http::Method::POST,
                axum::http::Method::OPTIONS,
            ])
            .allow_headers([
                axum::http::header::AUTHORIZATION,
                axum::http::header::CONTENT_TYPE,
                axum::http::header::HeaderName::from_static("x-csrf-token"),
            ]);
        // Wildcard + credentials is rejected by browsers and turns into an
        // origin-reflection hazard once a proxy rewrites the header (RT10).
        // Credentials are only enabled for an explicit origin allow-list.
        if !wildcard && !self.config.cors_origins.is_empty() {
            cors = cors.allow_credentials(true);
        }

        let layer = self.socket_io_layer.clone();
        let state = Arc::new(self);

        Router::new()
            .route("/health", get(health_check))
            .route("/stats", get(stats_handler))
            .layer(cors)
            .layer(layer)
            .with_state(state)
    }
}

/// Health check endpoint
async fn health_check() -> &'static str {
    "OK"
}

/// Returns `true` when the request carries a live Desk session.
///
/// The operational endpoints are not part of the guided API; they accept the
/// same cookie the Desk uses and read the one `tower-sessions` store. Fails
/// closed: no `AuthState`, no store, no cookie or an expired record all reject
/// rather than exposing rooms/presence anonymously.
async fn request_is_authorized(state: &RealtimeServer, headers: &HeaderMap) -> bool {
    let Some(auth) = &state.auth_state else {
        return false;
    };
    let Some(session_id) = ferris_auth::middleware::session_cookie(&auth.config, headers) else {
        return false;
    };
    let Some(store) = &auth.session_store else {
        return false;
    };
    store
        .session_identity(&session_id)
        .await
        .ok()
        .flatten()
        .is_some()
}

/// Stats endpoint (requires authentication)
async fn stats_handler(State(state): State<Arc<RealtimeServer>>, headers: HeaderMap) -> Response {
    if !request_is_authorized(&state, &headers).await {
        return (StatusCode::UNAUTHORIZED, "Authentication required").into_response();
    }

    let room_stats = state.room_manager.get_stats();
    let presence_stats = state.presence_manager.get_status_counts();

    // `json!` cannot panic on a request path; the previous three `unwrap()`s
    // relied on the serializers being infallible (RT11).
    axum::Json(serde_json::json!({
        "rooms": room_stats,
        "presence": presence_stats,
        "timestamp": Utc::now(),
    }))
    .into_response()
}

/// Start realtime server (convenience function)
pub async fn start_realtime_server(config: RealtimeConfig) -> Result<()> {
    let server = RealtimeServer::new(config);
    server.start().await
}

/// Process-wide handler used by the document-notification helpers below.
///
/// The realtime role is optional: when it is not mounted nothing installs a
/// handler and the helpers are no-ops, which is the correct behaviour for a
/// server that has no sockets. `ferris-server` installs the live handler when
/// it mounts the role; `ferris-api`'s `DocStore` calls the helpers after each
/// committed write. Before this wiring the whole `notify_doc_*` surface had no
/// production caller and a document write never reached a socket (RT2).
static HANDLER: OnceLock<RealtimeHandler> = OnceLock::new();

/// Install the process-wide realtime handler.
///
/// Returns `false` when one is already installed, mirroring
/// `ferris_wasm::set_realtime_broadcaster`; a second server in the same process
/// is a test/misconfiguration case and must not silently steal notifications.
///
/// Installing also registers the session-revocation listener, which is what
/// makes a `logout` (or "sign out everywhere", or a disabled account) close the
/// sockets that were opened with that credential instead of letting them keep
/// receiving documents until the token expires (RT-01).
pub fn install_handler(handler: RealtimeHandler) -> bool {
    if HANDLER.set(handler.clone()).is_err() {
        return false;
    }
    let live = handler;
    let _ = ferris_auth::middleware::install_session_revoked_hook(std::sync::Arc::new(
        move |user_id: &str, session_id: Option<&str>| {
            let closed = match session_id {
                Some(session_id) => live.disconnect_session(session_id),
                None => live.disconnect_user(user_id),
            };
            if closed > 0 {
                tracing::debug!(closed, user = %user_id, "realtime: closed revoked sockets");
            }
        },
    ));
    true
}

fn global_handler() -> Option<&'static RealtimeHandler> {
    HANDLER.get()
}

/// Close every realtime socket of `user_id`, returning how many were closed.
///
/// The session store already drives this through the installed revocation
/// hook; the free function is for a caller that revokes by user without a
/// session in hand (a disabled account, an administrative reset).
#[must_use]
pub fn disconnect_user(user_id: &str) -> usize {
    global_handler().map_or(0, |handler| handler.disconnect_user(user_id))
}

/// Close every realtime socket opened with `session_id`.
#[must_use]
pub fn disconnect_session(session_id: &str) -> usize {
    global_handler().map_or(0, |handler| handler.disconnect_session(session_id))
}

/// Notify subscribers that a document was inserted.
///
/// # Errors
///
/// Propagates an encoding or delivery failure from the live handler; a process
/// without a realtime handler returns `Ok(())`.
pub fn notify_doc_insert(doctype: &str, docname: &str, data: ferris_core::Value) -> Result<()> {
    match global_handler() {
        Some(handler) => handler.notify_doc_insert(doctype, docname, data),
        None => Ok(()),
    }
}

/// Notify subscribers that a document was updated.
///
/// # Errors
///
/// See [`notify_doc_insert`].
pub fn notify_doc_update(
    doctype: &str,
    docname: &str,
    data: ferris_core::Value,
    changes: ferris_core::Value,
    previous: ferris_core::Value,
) -> Result<()> {
    match global_handler() {
        Some(handler) => handler.notify_doc_update(doctype, docname, data, changes, previous),
        None => Ok(()),
    }
}

/// Notify subscribers that a document was deleted.
///
/// # Errors
///
/// See [`notify_doc_insert`].
pub fn notify_doc_delete(
    doctype: &str,
    docname: &str,
    previous: Option<ferris_core::Value>,
) -> Result<()> {
    match global_handler() {
        Some(handler) => handler.notify_doc_delete(
            doctype,
            docname,
            previous.unwrap_or(ferris_core::Value::Null),
        ),
        None => Ok(()),
    }
}

/// Notify subscribers that a document was submitted.
///
/// # Errors
///
/// See [`notify_doc_insert`].
pub fn notify_doc_submit(doctype: &str, docname: &str, data: ferris_core::Value) -> Result<()> {
    match global_handler() {
        Some(handler) => handler.notify_doc_submit(doctype, docname, data),
        None => Ok(()),
    }
}

/// Notify subscribers that a document was cancelled.
///
/// # Errors
///
/// See [`notify_doc_insert`].
pub fn notify_doc_cancel(doctype: &str, docname: &str, data: ferris_core::Value) -> Result<()> {
    match global_handler() {
        Some(handler) => handler.notify_doc_cancel(doctype, docname, data),
        None => Ok(()),
    }
}

/// Broadcast event to all connected clients
pub async fn broadcast_event(state: &Arc<RealtimeServer>, event: RealtimeEvent) -> Result<()> {
    state.handler.broadcast(&event)
}

/// Send event to specific user
pub async fn send_to_user(
    state: &Arc<RealtimeServer>,
    user_id: &str,
    event: RealtimeEvent,
) -> Result<()> {
    state.handler.send_to_user(user_id, &event)
}

/// Send event to room
pub async fn send_to_room(
    state: &Arc<RealtimeServer>,
    room_name: &str,
    event: RealtimeEvent,
) -> Result<()> {
    state.handler.send_to_room(room_name, &event)
}

/// Send event to doctype subscribers
pub async fn send_to_doctype(
    state: &Arc<RealtimeServer>,
    doctype: &str,
    event: RealtimeEvent,
) -> Result<()> {
    state.handler.send_to_doctype(doctype, &event)
}

/// Send event to list subscribers
pub async fn send_to_list(
    state: &Arc<RealtimeServer>,
    doctype: &str,
    filters_hash: &str,
    event: RealtimeEvent,
) -> Result<()> {
    state.handler.send_to_list(doctype, filters_hash, &event)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_auth_state() -> AuthState {
        AuthState::new(ferris_auth::SessionConfig::default())
    }

    #[tokio::test]
    async fn test_server_creation() {
        let config = RealtimeConfig::default();
        let server = RealtimeServer::new(config);

        assert_eq!(server.config.port, 8001);
    }

    /// V3-RT-1: `with_auth` must attach to the handler the Socket.IO namespace
    /// closure owns, not just to the server's stats gate. Before the fix the
    /// socket handler never received auth and every connection authenticated as
    /// anonymous.
    #[test]
    fn with_auth_reaches_the_live_socket_handler() {
        let server = RealtimeServer::new(RealtimeConfig::default());
        assert!(!server.handler.has_auth(), "no auth by default");

        let server = server.with_auth(test_auth_state());
        assert!(server.auth_state.is_some(), "stats gate sees auth");
        assert!(
            server.handler.has_auth(),
            "the live socket handler must see the same auth"
        );
    }

    #[test]
    fn test_room_helpers() {
        assert_eq!(crate::rooms::Room::user_room("user1").name, "user:user1");
        assert_eq!(
            crate::rooms::Room::doctype_room("Sales Order").name,
            "doctype:Sales Order"
        );
        assert_eq!(
            crate::rooms::Room::list_room("Sales Order", "abc123").name,
            "list:Sales Order:abc123"
        );
    }
}
