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
    /// the revocation hook re-check it.
    session_id: Option<String>,
    /// For a bearer connection, when the token stops being valid on its own.
    token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
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

        // Authenticate from the handshake (`Authorization: Bearer ...` or the
        // session cookie).
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
    /// The credential itself is kept on the returned record (its session id or
    /// the bearer token's expiry), because the handshake is only the first
    /// authorization: [`Self::revalidate_connections`] and the session
    /// revocation hook re-check it later (RT-01).
    async fn authenticate(&self, socket: &SocketRef) -> Option<ConnectionAuth> {
        // Clone the `AuthState` out of the shared cell before any `.await`:
        // the read guard is not `Send` and must not be held across the
        // `validator.validate(..).await` below (V3-RT-1).
        let auth = self.auth_state.read().clone()?;
        let headers = &socket.req_parts().headers;
        let (user_id, user_name, roles) = authenticate_headers(&auth, headers).await?;
        Some(ConnectionAuth {
            user_id,
            user_name,
            roles,
            session_id: ferris_auth::middleware::session_cookie(&auth.config, headers),
            token_expires_at: bearer_expiry(&auth, headers),
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
/// Bearer connections are checked against the token's expiry and the user
/// validator; cookie connections are checked against the session store, so a
/// deleted session closes its socket. A role change is treated as a revocation:
/// the socket re-handshakes and picks the new roles up, instead of keeping the
/// old ones for the connection's lifetime.
async fn connection_is_current(
    auth: &ferris_auth::AuthState,
    connection: &ConnectionAuth,
    now: chrono::DateTime<Utc>,
) -> bool {
    if connection
        .token_expires_at
        .is_some_and(|expires_at| expires_at <= now)
    {
        return false;
    }
    if let Some(session_id) = &connection.session_id {
        match auth.session_manager.get_session(session_id).await {
            Ok(Some(session)) => {
                // Absolute expiry: the idle window is refreshed by the Desk's
                // own session traffic, not by socket heartbeats, and a live
                // socket is activity in its own right.
                if session.user_id != connection.user_id || session.is_expired() {
                    return false;
                }
            }
            Ok(None) => return false,
            Err(error) => {
                tracing::warn!(%error, "realtime: session check failed; closing the socket");
                return false;
            }
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

/// The expiry of the bearer token in `headers`, when there is one.
///
/// A token that does not validate is not a credential; the handshake has
/// already refused it, so `None` here only means "this is a cookie
/// connection".
fn bearer_expiry(
    auth: &ferris_auth::AuthState,
    headers: &axum::http::HeaderMap,
) -> Option<chrono::DateTime<Utc>> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))?;
    let claims = auth.jwt_manager.validate_access_token(token).ok()?;
    chrono::DateTime::from_timestamp(claims.exp, 0)
}

/// Resolve the handshake identity from its headers.
///
/// Two credentials are accepted, in this order:
///
/// 1. `Authorization: Bearer <jwt>` — the explicit credential a native client,
///    the CLI or a plugin uses;
/// 2. the Desk's **session cookie** — what a browser tab actually has. Without
///    this the Desk's own realtime connection was anonymous: the browser never
///    sends a bearer token, so presence, the personal `user:` room and every
///    `session_can` decision ran as a guest.
///
/// No credential at all stays anonymous, exactly as before.
///
/// The cookie goes through the same [`SessionManager`](ferris_auth::SessionManager)
/// the REST middleware uses (`touch_session` also refuses an expired or idle
/// session), and the identity it yields is validated the same way a bearer
/// token's is, so a disabled account cannot hold a socket open. When no
/// `UserValidator` is configured the session's own stored identity is the
/// answer, matching what `GET /api/auth/session` reports.
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
) -> Option<(String, String, Vec<String>)> {
    if let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
    {
        return authenticate_token(auth, token).await;
    }
    let session_id = ferris_auth::middleware::session_cookie(&auth.config, headers)?;
    authenticate_session(auth, &session_id).await
}

/// The bearer half: validate the JWT, then resolve the user (roles included)
/// through the validator when one is configured.
async fn authenticate_token(
    auth: &ferris_auth::AuthState,
    token: &str,
) -> Option<(String, String, Vec<String>)> {
    let claims = auth.jwt_manager.validate_access_token(token).ok()?;
    if let Some(validator) = &auth.user_validator {
        let user = validator.validate(&claims.sub).await?;
        if !user.enabled {
            return None;
        }
        return Some((user.id, user.email, user.roles));
    }
    Some((claims.sub, claims.email, claims.roles))
}

/// The cookie half: the session is the credential, so it is loaded through the
/// shared manager (which also refuses an expired/idle one) and then resolved to
/// an identity the same way a token is.
async fn authenticate_session(
    auth: &ferris_auth::AuthState,
    session_id: &str,
) -> Option<(String, String, Vec<String>)> {
    let session = auth
        .session_manager
        .touch_session(session_id)
        .await
        .ok()??;
    if let Some(validator) = &auth.user_validator {
        let user = validator.validate(&session.user_id).await?;
        if !user.enabled {
            return None;
        }
        return Some((user.id, user.email, user.roles));
    }
    // No source-of-truth lookup installed: report what the session recorded,
    // which is the same fallback `GET /api/auth/session` uses.
    let email = session
        .get_data("email")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let roles = session
        .get_data("roles")
        .and_then(|value| value.as_array())
        .map(|roles| {
            roles
                .iter()
                .filter_map(|role| role.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some((session.user_id, email, roles))
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

    fn auth_state() -> AuthState {
        let jwt = ferris_auth::JwtManager::new(
            ferris_auth::JwtConfig::new("ferris-realtime-test-secret-32-chars+")
                .expect("valid test secret"),
        )
        .expect("jwt manager");
        let config = ferris_auth::SessionConfig::default();
        AuthState::new(
            jwt,
            ferris_auth::SessionManager::new(config.clone()),
            config,
        )
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
        token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        handler.connections.write().insert(
            connection_id.to_string(),
            ConnectionAuth {
                user_id: user_id.to_string(),
                user_name: user_id.to_string(),
                roles: vec!["User".to_string()],
                session_id: session_id.map(str::to_string),
                token_expires_at,
            },
        );
    }

    /// RT-01: `logout` / "sign out everywhere" must close the sockets, not only
    /// delete the row the next HTTP request reads.
    #[tokio::test]
    async fn disconnect_user_closes_every_socket_of_the_user() {
        let handler = handler_with_auth(auth_state());
        insert(
            &handler,
            "aaaaaaaaaaaaaaaa",
            "ada@example.com",
            Some("s1"),
            None,
        );
        insert(
            &handler,
            "bbbbbbbbbbbbbbbb",
            "ada@example.com",
            Some("s2"),
            None,
        );
        insert(
            &handler,
            "cccccccccccccccc",
            "bob@example.com",
            Some("s3"),
            None,
        );

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
        let handler = handler_with_auth(auth_state());
        insert(
            &handler,
            "aaaaaaaaaaaaaaaa",
            "ada@example.com",
            Some("s1"),
            None,
        );
        insert(
            &handler,
            "bbbbbbbbbbbbbbbb",
            "ada@example.com",
            Some("s2"),
            None,
        );

        assert_eq!(handler.disconnect_session("s1"), 1);
        let live = handler.connections.read();
        assert!(!live.contains_key("aaaaaaaaaaaaaaaa"));
        assert!(live.contains_key("bbbbbbbbbbbbbbbb"));
    }

    /// RT-01: a socket whose session was deleted must not keep receiving
    /// documents; the maintenance pass closes it.
    #[tokio::test]
    async fn a_revoked_session_closes_its_socket() {
        let auth = auth_state();
        let session = auth
            .session_manager
            .create_session("ada@example.com".to_string(), None, None)
            .await
            .expect("session");
        let handler = handler_with_auth(auth.clone());
        insert(
            &handler,
            "aaaaaaaaaaaaaaaa",
            "ada@example.com",
            Some(&session.id),
            None,
        );

        assert_eq!(
            handler.revalidate_connections().await,
            0,
            "a live session keeps the socket open"
        );

        auth.session_manager
            .delete_session(&session.id)
            .await
            .expect("logout");
        assert_eq!(handler.revalidate_connections().await, 1);
        assert!(handler.connections.read().is_empty());
    }

    /// RT-01: an access token that expired after the handshake closes its
    /// socket, even for a connection with no session behind it.
    #[tokio::test]
    async fn an_expired_token_closes_its_socket() {
        let handler = handler_with_auth(auth_state());
        insert(
            &handler,
            "aaaaaaaaaaaaaaaa",
            "ada@example.com",
            None,
            Some(Utc::now() - chrono::Duration::minutes(1)),
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

        let handler = handler_with_auth(auth_state().with_user_validator(Arc::new(CurrentRoles)));
        handler.connections.write().insert(
            "aaaaaaaaaaaaaaaa".to_string(),
            ConnectionAuth {
                user_id: "ada@example.com".to_string(),
                user_name: "Ada".to_string(),
                roles: vec!["User".to_string(), "System Manager".to_string()],
                session_id: None,
                token_expires_at: None,
            },
        );
        assert_eq!(handler.revalidate_connections().await, 1);
    }
}
