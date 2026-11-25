//! Socket.IO event handler.
//!
//! Split into focused files so no one file carries both the connection
//! lifecycle and the emit surface: `authz` holds the DocType-room gate,
//! `subscriptions` the per-socket event handlers, `emit` the notification
//! helpers, and `tests` the regression suite. Paths are preserved by the
//! re-exports below (`crate::handler::DocSubscription` still resolves).

mod authz;
mod emit;
mod subscriptions;

#[cfg(test)]
mod tests;

pub use subscriptions::{
    ChannelSubscription, CommentSubscription, DocSubscription, DocUnsubscription, ListSubscription,
    ListUnsubscription, SetStatusData, StatusMessageData, StopTypingData, TypingData,
};

use crate::{PresenceManager, RoomManager};
use chrono::Utc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use socketioxide::{SocketIo, extract::SocketRef};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

use ferris_auth::AuthState;

/// Authenticated identity attached to a connection.
#[derive(Debug, Clone)]
struct ConnectionAuth {
    user_id: String,
    user_name: String,
    roles: Vec<String>,
    /// The session a cookie-authenticated socket was opened with.
    ///
    /// A socket used to keep the handshake's authorization for its whole life,
    /// so a logout, a disabled account or a withdrawn role never interrupted
    /// delivery (RT-01). Recording the credential lets the maintenance task and
    /// the revocation hook re-check it against the one `tower-sessions` store.
    session_id: Option<String>,
}

/// Connection information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub connection_id: String,
    pub user_id: Option<String>,
    pub user_name: Option<String>,
    pub tenant_id: Option<String>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub connected_at: chrono::DateTime<chrono::Utc>,
    pub last_activity: chrono::DateTime<chrono::Utc>,
    pub rooms: Vec<String>,
    pub metadata: std::collections::HashMap<String, String>,
}

/// Realtime handler for Socket.IO events
#[derive(Clone)]
pub struct RealtimeHandler {
    room_manager: Arc<RoomManager>,
    presence_manager: Arc<PresenceManager>,
    /// Shared so the server can attach auth **after** the handler has been
    /// moved into the socket.io connection closure (V3-RT-1). Before this the
    /// `with_auth` call set a different field than the one `authenticate`
    /// read, so every socket was anonymous.
    auth_state: Arc<RwLock<Option<AuthState>>>,
    connections: Arc<RwLock<HashMap<String, ConnectionAuth>>>,
    socket_io: Option<SocketIo>,
}

impl RealtimeHandler {
    pub fn new(room_manager: Arc<RoomManager>, presence_manager: Arc<PresenceManager>) -> Self {
        Self {
            room_manager,
            presence_manager,
            auth_state: Arc::new(RwLock::new(None)),
            connections: Arc::new(RwLock::new(HashMap::new())),
            socket_io: None,
        }
    }

    pub fn with_auth(self, auth_state: ferris_auth::AuthState) -> Self {
        self.set_auth(auth_state);
        self
    }

    /// Attach (or replace) auth through the shared cell that `authenticate`
    /// reads, so it works even after the handler was cloned into the closure.
    pub fn set_auth(&self, auth_state: ferris_auth::AuthState) {
        *self.auth_state.write() = Some(auth_state);
    }

    /// Whether an [`ferris_auth::AuthState`] has been attached.
    ///
    /// Because `auth_state` lives behind an `Arc`, this is `true` for every
    /// clone once any clone has been given auth (V3-RT-1).
    pub fn has_auth(&self) -> bool {
        self.auth_state.read().is_some()
    }

    /// Attach the Socket.IO instance so room broadcasts are actually delivered.
    pub fn with_socket_io(mut self, io: SocketIo) -> Self {
        self.socket_io = Some(io);
        self
    }

    /// Handle new connection.
    pub async fn on_connect(&self, socket: SocketRef) {
        let connection_id = socket.id.to_string();
        debug!("New connection: {}", connection_id);

        // Authenticate from the handshake's session cookie.
        if let Some(auth) = self.authenticate(&socket).await {
            let user_id = auth.user_id.clone();
            let user_name = auth.user_name.clone();
            self.connections.write().insert(connection_id.clone(), auth);
            let room_name = format!("user:{}", user_id);
            // Doctype rooms default to deny; only the personal user room is joined here.
            let _ = self
                .room_manager
                .join_authorized(&room_name, &connection_id, &user_id, |_| false);
            // Mirror the authorization in Socket.IO's own room registry so
            // `send_to_user`/`send_to_room` actually deliver (V3-RT-2).
            let _ = socket.join(room_name.clone());

            let conn_info = crate::presence::PresenceConnection {
                connection_id: connection_id.clone(),
                ip_address: None,
                user_agent: None,
                device: None,
                browser: None,
                os: None,
                connected_at: Utc::now(),
                last_activity: Utc::now(),
                rooms: vec![room_name.clone()],
            };

            let _ = self
                .presence_manager
                .user_connected(user_id, user_name, conn_info);
        }

        // Register event handlers
        self.register_handlers(socket);
    }

    /// Resolve the authenticated user from the socket handshake.
    ///
    /// Returns `None` for anonymous connections and for users rejected by the
    /// configured [`ferris_auth::UserValidator`] (e.g. disabled accounts).
    ///
    /// The credential itself is kept on the returned record (its session id),
    /// because the handshake is only the first authorization:
    /// [`Self::revalidate_connections`] and the session revocation hook
    /// re-check it later (RT-01).
    async fn authenticate(&self, socket: &SocketRef) -> Option<ConnectionAuth> {
        // Clone the `AuthState` out of the shared cell before any `.await`:
        // the read guard is not `Send` and must not be held across the
        // `validator.validate(..).await` below (V3-RT-1).
        let auth = self.auth_state.read().clone()?;
        let headers = &socket.req_parts().headers;
        let identity = authenticate_headers(&auth, headers).await?;
        Some(ConnectionAuth {
            user_id: identity.user_id,
            user_name: identity.email,
            roles: identity.roles,
            session_id: Some(identity.session_id),
        })
    }

    /// Close every socket of `user_id`, returning how many were closed.
    ///
    /// This is the realtime half of a revocation: `logout`, "sign out
    /// everywhere" and a disabled account must stop the delivery, not only the
    /// next HTTP request (RT-01). The session store calls it through
    /// [`ferris_auth::middleware::install_session_revoked_hook`]; a caller can
    /// also use the free [`crate::server::disconnect_user`].
    #[must_use]
    pub fn disconnect_user(&self, user_id: &str) -> usize {
        let ids: Vec<String> = self
            .connections
            .read()
            .iter()
            .filter(|(_, auth)| auth.user_id == user_id)
            .map(|(connection_id, _)| connection_id.clone())
            .collect();
        for connection_id in &ids {
            self.drop_connection(connection_id);
        }
        ids.len()
    }

    /// Close every socket opened with `session_id`, returning how many were
    /// closed.
    #[must_use]
    pub fn disconnect_session(&self, session_id: &str) -> usize {
        let ids: Vec<String> = self
            .connections
            .read()
            .iter()
            .filter(|(_, auth)| auth.session_id.as_deref() == Some(session_id))
            .map(|(connection_id, _)| connection_id.clone())
            .collect();
        for connection_id in &ids {
            self.drop_connection(connection_id);
        }
        ids.len()
    }

    /// Re-check every live connection against the current authorization.
    ///
    /// The handshake captures the identity once; without this a socket keeps
    /// receiving documents after the session is deleted, the account is
    /// disabled, the token expires or a role is withdrawn (RT-01). Called from
    /// the maintenance task the server starts when it mounts the realtime
    /// layer. A connection that no longer matches is closed, and its client
    /// re-handshakes with whatever credential it still has.
    ///
    /// Returns how many connections were closed.
    pub async fn revalidate_connections(&self) -> usize {
        // Clone the state out of the lock before any `.await`.
        let Some(auth) = self.auth_state.read().clone() else {
            return 0;
        };
        let snapshot: Vec<(String, ConnectionAuth)> = self
            .connections
            .read()
            .iter()
            .map(|(connection_id, auth)| (connection_id.clone(), auth.clone()))
            .collect();
        let now = Utc::now();
        let mut closed = 0;
        for (connection_id, connection) in snapshot {
            if !connection_is_current(&auth, &connection, now).await {
                tracing::debug!(
                    connection = %connection_id,
                    user = %connection.user_id,
                    "closing a realtime connection whose authorization was revoked"
                );
                self.drop_connection(&connection_id);
                closed += 1;
            }
        }
        closed
    }

    /// Forget one connection and ask Socket.IO to close it.
    ///
    /// The local bookkeeping is cleared first, so the state is correct even
    /// when no engine was attached (unit tests, an embedded handler); the
    /// disconnect packet is best-effort and the client's own reconnection
    /// re-runs the handshake.
    fn drop_connection(&self, connection_id: &str) {
        let _ = self.room_manager.leave_all(connection_id);
        let _ = self.presence_manager.user_disconnected(connection_id);
        self.connections.write().remove(connection_id);
        let Some(io) = &self.socket_io else {
            return;
        };
        match connection_id.parse::<socketioxide::socket::Sid>() {
            Ok(sid) => {
                if let Some(socket) = io.get_socket(sid)
                    && let Err(error) = socket.disconnect()
                {
                    debug!("could not close socket {connection_id}: {error}");
                }
            }
            Err(error) => debug!("unknown socket id {connection_id}: {error}"),
        }
    }
}

/// Whether a connection still holds the authorization it was opened with.
///
/// A connection is checked against the session store, so a deleted session
/// closes its socket, and against the user validator, so a disabled account or
/// a role change is treated as a revocation: the socket re-handshakes and picks
/// the new roles up instead of keeping the old ones for the connection's
/// lifetime.
async fn connection_is_current(
    auth: &ferris_auth::AuthState,
    connection: &ConnectionAuth,
    _now: chrono::DateTime<Utc>,
) -> bool {
    if let Some(session_id) = &connection.session_id {
        let Some(store) = &auth.session_store else {
            // No store means this process cannot prove the session still
            // exists; a socket must not outlive its credential.
            return false;
        };
        if !store.is_active(session_id, &connection.user_id).await {
            return false;
        }
    }
    match &auth.user_validator {
        Some(validator) => match validator.validate(&connection.user_id).await {
            Some(user) => user.enabled && user.roles == connection.roles,
            None => false,
        },
        None => true,
    }
}

/// Resolve the handshake identity from its headers.
///
/// The Desk's browser sends the session cookie; there is no bearer credential
/// any more. The socket upgrade is handled before the HTTP auth layers, so the
/// identity is read from the same `tower-sessions` record by id
/// ([`DbSessionStore::session_identity`](ferris_auth::login::DbSessionStore::session_identity)).
/// When a [`UserValidator`](ferris_auth::middleware::UserValidator) is
/// configured its answer is the current one — a disabled account is refused at
/// the handshake and a role change is adopted here rather than waiting for the
/// maintenance tick.
///
/// No credential at all stays anonymous, exactly as before.
///
/// Residual risk, deliberately not papered over here: a cross-site WebSocket
/// hijack would need the browser to attach the session cookie to a handshake
/// initiated by another origin. The default `SameSite=Lax` session cookie is
/// not sent on such a request; a deployment that configures
/// `SameSite=None` removes that protection and should front this with an
/// origin allow-list, which needs the server's CORS settings (not available to
/// this crate).
pub(super) async fn authenticate_headers(
    auth: &ferris_auth::AuthState,
    headers: &axum::http::HeaderMap,
) -> Option<ferris_auth::login::SessionIdentity> {
    let session_id = ferris_auth::middleware::session_cookie(&auth.config, headers)?;
    let store = auth.session_store.as_ref()?;
    let mut identity = store.session_identity(&session_id).await.ok()??;
    if let Some(validator) = &auth.user_validator {
        let user = validator.validate(&identity.user_id).await?;
        if !user.enabled {
            return None;
        }
        identity.email = user.email;
        identity.roles = user.roles;
    }
    Some(identity)
}

/// Regression tests for the connection lifecycle added with RT-01.
///
/// Kept inline (and under their own name) because `handler/tests.rs` is a
/// separate file this change does not own; these tests need the private
/// `connections` map that only this module can populate without a socket.
#[cfg(test)]
mod connection_lifecycle_tests {
    use super::*;
    use crate::{PresenceManager, RoomManager};
    use std::sync::Arc;

    async fn auth_state() -> (AuthState, ferris_auth::login::DbSessionStore) {
        let pool = ferris_db::memory_db().await.expect("in-memory db");
        let store = ferris_auth::login::DbSessionStore::new(pool);
        store.ensure_table().await.expect("session table");
        let config = ferris_auth::SessionConfig::default();
        (
            AuthState::new(config).with_session_store(store.clone()),
            store,
        )
    }

    /// Write a live session record carrying the identity the login stamps.
    async fn seed_session(store: &ferris_auth::login::DbSessionStore, user_id: &str) -> String {
        use tower_sessions::SessionStore;
        let mut record = tower_sessions::session::Record {
            id: Default::default(),
            data: std::collections::HashMap::new(),
            expiry_date: time::OffsetDateTime::now_utc() + time::Duration::hours(1),
        };
        record.data.insert(
            ferris_auth::login::keys::USER_ID_KEY.to_string(),
            serde_json::json!(user_id),
        );
        record.data.insert(
            ferris_auth::login::keys::EMAIL_KEY.to_string(),
            serde_json::json!(user_id),
        );
        record.data.insert(
            ferris_auth::login::keys::ROLES_KEY.to_string(),
            serde_json::json!(["User"]),
        );
        store.create(&mut record).await.expect("session record");
        record.id.to_string()
    }

    fn handler_with_auth(auth: AuthState) -> RealtimeHandler {
        RealtimeHandler::new(
            Arc::new(RoomManager::new()),
            Arc::new(PresenceManager::new()),
        )
        .with_auth(auth)
    }

    fn insert(
        handler: &RealtimeHandler,
        connection_id: &str,
        user_id: &str,
        session_id: Option<&str>,
    ) {
        handler.connections.write().insert(
            connection_id.to_string(),
            ConnectionAuth {
                user_id: user_id.to_string(),
                user_name: user_id.to_string(),
                roles: vec!["User".to_string()],
                session_id: session_id.map(str::to_string),
            },
        );
    }

    /// RT-01: `logout` / "sign out everywhere" must close the sockets, not only
    /// delete the row the next HTTP request reads.
    #[tokio::test]
    async fn disconnect_user_closes_every_socket_of_the_user() {
        let (auth, _store) = auth_state().await;
        let handler = handler_with_auth(auth);
        insert(&handler, "aaaaaaaaaaaaaaaa", "ada@example.com", Some("s1"));
        insert(&handler, "bbbbbbbbbbbbbbbb", "ada@example.com", Some("s2"));
        insert(&handler, "cccccccccccccccc", "bob@example.com", Some("s3"));

        assert_eq!(handler.disconnect_user("ada@example.com"), 2);
        let live = handler.connections.read();
        assert!(!live.contains_key("aaaaaaaaaaaaaaaa"));
        assert!(!live.contains_key("bbbbbbbbbbbbbbbb"));
        assert!(
            live.contains_key("cccccccccccccccc"),
            "another user's socket stays"
        );
    }

    #[tokio::test]
    async fn disconnect_session_closes_only_that_credential() {
        let (auth, _store) = auth_state().await;
        let handler = handler_with_auth(auth);
        insert(&handler, "aaaaaaaaaaaaaaaa", "ada@example.com", Some("s1"));
        insert(&handler, "bbbbbbbbbbbbbbbb", "ada@example.com", Some("s2"));

        assert_eq!(handler.disconnect_session("s1"), 1);
        let live = handler.connections.read();
        assert!(!live.contains_key("aaaaaaaaaaaaaaaa"));
        assert!(live.contains_key("bbbbbbbbbbbbbbbb"));
    }

    /// RT-01: a socket whose session was deleted must not keep receiving
    /// documents; the maintenance pass closes it.
    #[tokio::test]
    async fn a_revoked_session_closes_its_socket() {
        use tower_sessions::SessionStore;
        let (auth, store) = auth_state().await;
        let session_id = seed_session(&store, "ada@example.com").await;
        let handler = handler_with_auth(auth.clone());
        insert(
            &handler,
            "aaaaaaaaaaaaaaaa",
            "ada@example.com",
            Some(&session_id),
        );

        assert_eq!(
            handler.revalidate_connections().await,
            0,
            "a live session keeps the socket open"
        );

        let id = session_id.parse().expect("session id");
        store.delete(&id).await.expect("logout");
        assert_eq!(handler.revalidate_connections().await, 1);
        assert!(handler.connections.read().is_empty());
    }

    /// A socket with no session record to prove cannot stay open: the
    /// realtime path has no bearer fallback any more.
    #[tokio::test]
    async fn a_connection_without_a_live_session_closes() {
        let (auth, _store) = auth_state().await;
        let handler = handler_with_auth(auth);
        insert(
            &handler,
            "aaaaaaaaaaaaaaaa",
            "ada@example.com",
            Some("not-a-session"),
        );
        assert_eq!(handler.revalidate_connections().await, 1);
    }

    /// RT-01: a withdrawn role re-handshakes instead of being kept for the
    /// connection's whole life.
    #[tokio::test]
    async fn a_withdrawn_role_closes_the_socket() {
        struct CurrentRoles;
        #[async_trait::async_trait]
        impl ferris_auth::middleware::UserValidator for CurrentRoles {
            async fn validate(&self, user_id: &str) -> Option<ferris_core::User> {
                let mut user =
                    ferris_core::User::system_user(user_id.to_string(), user_id.to_string());
                user.roles = vec!["User".to_string()];
                Some(user)
            }
        }

        let (auth, _store) = auth_state().await;
        let handler = handler_with_auth(auth.with_user_validator(Arc::new(CurrentRoles)));
        handler.connections.write().insert(
            "aaaaaaaaaaaaaaaa".to_string(),
            ConnectionAuth {
                user_id: "ada@example.com".to_string(),
                user_name: "Ada".to_string(),
                roles: vec!["User".to_string(), "System Manager".to_string()],
                session_id: None,
            },
        );
        assert_eq!(handler.revalidate_connections().await, 1);
    }
}
