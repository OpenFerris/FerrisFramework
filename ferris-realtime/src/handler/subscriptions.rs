//! Per-socket event handlers and their wire payloads.

use super::{ConnectionAuth, RealtimeHandler};
use serde::{Deserialize, Serialize};
use socketioxide::extract::{Data, SocketRef};
use std::collections::HashMap;
use tracing::{debug, warn};

impl RealtimeHandler {
    // Each `socket.on(...)` hook is a self-contained block (join a room,
    // answer an error, emit an ack); keeping them in one place preserves the
    // subscribe/unsubscribe symmetry at a glance.
    #[allow(clippy::too_many_lines)]
    pub(super) fn register_handlers(&self, socket: SocketRef) {
        // Subscribe to a doctype room (requires read access to the doctype).
        //
        // The room is deliberately DocType-wide: `notify_doc_insert`/`update`/
        // `delete` emit to `doctype:<DocType>` with the whole document, so a
        // per-document subscription could not be honoured by that emit path.
        // `DocSubscription` therefore names only the DocType (RT5).
        let rooms = self.room_manager.clone();
        let conns = self.connections.clone();
        socket.on(
            "subscribe-doc",
            move |socket: SocketRef, Data(sub): Data<DocSubscription>| {
                let cid = socket.id.to_string();
                let room = format!("doctype:{}", sub.doctype);
                let Some(auth) = conns.read().get(&cid).cloned() else {
                    let _ = socket.emit(
                        "subscribe-error",
                        serde_json::json!({ "error": "unauthenticated", "room": room }),
                    );
                    return;
                };
                let allowed =
                    RealtimeHandler::can_read_doctype(&auth.user_id, &auth.roles, &sub.doctype);
                match rooms.join_authorized(&room, &cid, &auth.user_id, |_| allowed) {
                    Ok(()) => {
                        // Mirror into Socket.IO so delivery uses the same room
                        // registry the emit path targets (V3-RT-2).
                        let _ = socket.join(room.clone());
                        let _ = socket.emit("subscribed", serde_json::json!({ "room": room }));
                    }
                    Err(_) => {
                        let _ = socket.emit(
                            "subscribe-error",
                            serde_json::json!({ "error": "forbidden", "room": room }),
                        );
                    }
                }
            },
        );

        // Subscribe to a filtered list room (also requires read access).
        let rooms = self.room_manager.clone();
        let conns = self.connections.clone();
        socket.on(
            "subscribe-list",
            move |socket: SocketRef, Data(sub): Data<ListSubscription>| {
                let cid = socket.id.to_string();
                let room = format!("list:{}:{}", sub.doctype, sub.filters_hash);
                let Some(auth) = conns.read().get(&cid).cloned() else {
                    let _ = socket.emit(
                        "subscribe-error",
                        serde_json::json!({ "error": "unauthenticated", "room": room }),
                    );
                    return;
                };
                let allowed =
                    RealtimeHandler::can_read_doctype(&auth.user_id, &auth.roles, &sub.doctype);
                match rooms.join_authorized(&room, &cid, &auth.user_id, |_| allowed) {
                    Ok(()) => {
                        let _ = socket.join(room.clone());
                        let _ = socket.emit("subscribed", serde_json::json!({ "room": room }));
                    }
                    Err(_) => {
                        let _ = socket.emit(
                            "subscribe-error",
                            serde_json::json!({ "error": "forbidden", "room": room }),
                        );
                    }
                }
            },
        );

        // Subscribe to a page's comment stream.
        //
        // The room used to be public: an anonymous socket could join
        // `comments:<anything>` with no credential and no route check (RT-02).
        // Comments are only ever published to signed-in readers, so the
        // subscription now requires an authenticated connection and a route
        // that is a local path (not a URL, not traversal), mirroring the
        // `next` guard of the OAuth flow.
        let rooms = self.room_manager.clone();
        let conns = self.connections.clone();
        socket.on(
            "subscribe-comments",
            move |socket: SocketRef, Data(sub): Data<CommentSubscription>| {
                let cid = socket.id.to_string();
                let room = match authorize_comment_room(&conns.read(), &cid, &sub.route) {
                    Ok(room) => room,
                    Err(error) => {
                        let _ =
                            socket.emit("subscribe-error", serde_json::json!({ "error": error }));
                        return;
                    }
                };
                // The route is validated and the caller signed in; the room
                // itself carries no per-document authorization.
                let _ = rooms.join(&room, &cid);
                let _ = socket.join(room.clone());
                let _ = socket.emit("subscribed", serde_json::json!({ "room": room }));
            },
        );

        // Subscribe to a plugin pubsub channel (RT-03).
        //
        // `io::publish` fans out to `channel:<c>`, but nothing ever joined that
        // room, so the socket.io half of the publication was a silent no-op.
        // The scope rule is the SSE endpoint's: a caller may only listen to its
        // own `user:<id>` namespace and its children, because the payload is
        // raw plugin JSON and a channel name is not a capability.
        let rooms = self.room_manager.clone();
        let conns = self.connections.clone();
        socket.on(
            "subscribe-channel",
            move |socket: SocketRef, Data(sub): Data<ChannelSubscription>| {
                let cid = socket.id.to_string();
                let room = match authorize_channel_room(&conns.read(), &cid, &sub.channel) {
                    Ok(room) => room,
                    Err(error) => {
                        let _ =
                            socket.emit("subscribe-error", serde_json::json!({ "error": error }));
                        return;
                    }
                };
                let _ = rooms.join(&room, &cid);
                let _ = socket.join(room.clone());
                let _ = socket.emit("subscribed", serde_json::json!({ "room": room }));
            },
        );

        // Unsubscribe from document
        let rooms = self.room_manager.clone();
        socket.on(
            "unsubscribe-doc",
            move |socket: SocketRef, Data(sub): Data<DocUnsubscription>| {
                let cid = socket.id.to_string();
                let room = format!("doctype:{}", sub.doctype);
                let _ = rooms.leave(&room, &cid);
                let _ = socket.leave(room);
            },
        );

        // Unsubscribe from list
        let rooms = self.room_manager.clone();
        socket.on(
            "unsubscribe-list",
            move |socket: SocketRef, Data(sub): Data<ListUnsubscription>| {
                let cid = socket.id.to_string();
                let room = format!("list:{}:{}", sub.doctype, sub.filters_hash);
                let _ = rooms.leave(&room, &cid);
                let _ = socket.leave(room);
            },
        );

        // Typing indicator. Forwarded to the DocType room, with the identity
        // taken from the authenticated connection — never from the payload,
        // which a client could spoof (RT4).
        let handler = self.clone();
        socket.on(
            "typing",
            move |socket: SocketRef, Data(data): Data<TypingData>| {
                handler.forward_typing(&socket, data, true);
            },
        );

        // Stop typing
        let handler = self.clone();
        socket.on(
            "stop-typing",
            move |socket: SocketRef, Data(data): Data<StopTypingData>| {
                handler.forward_stop_typing(&socket, data);
            },
        );

        // Ping/pong for heartbeat
        socket.on("ping", |socket: SocketRef| {
            socket.emit("pong", serde_json::json!({})).ok();
        });

        // Update activity. This is what keeps the presence record and the
        // per-connection `last_activity` current (RT4/RT14).
        let presence = self.presence_manager.clone();
        socket.on("activity", move |socket: SocketRef| {
            presence.update_activity(&socket.id.to_string());
        });

        // Presence status changes. The user id comes from the connection, not
        // from the client payload.
        let presence = self.presence_manager.clone();
        let conns = self.connections.clone();
        socket.on(
            "set-status",
            move |socket: SocketRef, Data(data): Data<SetStatusData>| {
                let cid = socket.id.to_string();
                let Some(auth) = conns.read().get(&cid).cloned() else {
                    warn!("set-status from unauthenticated connection {cid}");
                    return;
                };
                if let Err(e) =
                    presence.set_status(&auth.user_id, data.status, data.message.clone())
                {
                    warn!("set-status failed for {}: {e}", auth.user_id);
                }
            },
        );

        let presence = self.presence_manager.clone();
        let conns = self.connections.clone();
        socket.on(
            "set-status-message",
            move |socket: SocketRef, Data(data): Data<StatusMessageData>| {
                let cid = socket.id.to_string();
                let Some(auth) = conns.read().get(&cid).cloned() else {
                    warn!("set-status-message from unauthenticated connection {cid}");
                    return;
                };
                if let Err(e) = presence.set_status_message(&auth.user_id, data.message) {
                    warn!("set-status-message failed for {}: {e}", auth.user_id);
                }
            },
        );

        // Disconnect handler
        let handler = self.clone();
        socket.on_disconnect(move |socket: SocketRef| {
            let connection_id = socket.id.to_string();
            debug!("Connection disconnected: {}", connection_id);
            let _ = handler.room_manager.leave_all(&connection_id);
            let _ = handler.presence_manager.user_disconnected(&connection_id);
            handler.connections.write().remove(&connection_id);
        });
    }

    /// Forward a typing indicator to the DocType room.
    ///
    /// The authenticated identity is authoritative: any `user_id`/`user_name`
    /// the client sent in the payload is ignored so a client cannot attribute
    /// activity to somebody else. Delivery is refused when the sender may not
    /// read the DocType, mirroring the subscription gate.
    fn forward_typing(&self, socket: &SocketRef, data: TypingData, typing: bool) {
        let Some(auth) = self.connection_auth(socket) else {
            warn!("typing event from unauthenticated connection");
            return;
        };
        if !Self::can_read_doctype(&auth.user_id, &auth.roles, &data.doctype) {
            warn!(
                "typing event refused: {} cannot read {}",
                auth.user_id, data.doctype
            );
            return;
        }
        let payload = serde_json::json!({
            "doctype": data.doctype,
            "docname": data.docname,
            "field": data.field,
            "user_id": auth.user_id,
            "user_name": auth.user_name,
        });
        self.emit_to_room(
            &format!("doctype:{}", data.doctype),
            if typing { "typing" } else { "stop-typing" },
            payload,
        );
    }

    /// Forward a "stopped typing" indicator, with the same authorization and
    /// identity rules as [`Self::forward_typing`].
    fn forward_stop_typing(&self, socket: &SocketRef, data: StopTypingData) {
        let Some(auth) = self.connection_auth(socket) else {
            warn!("stop-typing event from unauthenticated connection");
            return;
        };
        if !Self::can_read_doctype(&auth.user_id, &auth.roles, &data.doctype) {
            warn!(
                "stop-typing refused: {} cannot read {}",
                auth.user_id, data.doctype
            );
            return;
        }
        let payload = serde_json::json!({
            "doctype": data.doctype,
            "docname": data.docname,
            "field": data.field,
            "user_id": auth.user_id,
        });
        self.emit_to_room(&format!("doctype:{}", data.doctype), "stop-typing", payload);
    }

    fn connection_auth(&self, socket: &SocketRef) -> Option<ConnectionAuth> {
        let cid = socket.id.to_string();
        self.connections.read().get(&cid).cloned()
    }

    /// Emit one packet to a Socket.IO room, logging a transport failure instead
    /// of discarding it silently.
    fn emit_to_room(&self, room: &str, event: &str, payload: serde_json::Value) {
        let Some(io) = &self.socket_io else {
            return;
        };
        if let Err(e) = io.to(room.to_string()).emit(event.to_string(), payload) {
            warn!("realtime delivery to {room} failed: {e}");
        }
    }
}

/// Subscription data structures
///
/// A document subscription is DocType-wide: the emitter only produces
/// `EventTarget::Doctype`, so there is no per-document room to name (RT5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocSubscription {
    pub doctype: String,
}

/// Subscribe to a website route's comment stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentSubscription {
    pub route: String,
}

/// Subscribe to a plugin pubsub channel (`channel:<channel>` room).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSubscription {
    pub channel: String,
}

/// The longest website route a comment subscription may name.
const MAX_COMMENT_ROUTE_LEN: usize = 512;

/// A comment route that is a local absolute path, or `None`.
///
/// The room name is built from this string, so it must not be able to name
/// another origin (`//evil.example`), carry a scheme, a fragment, control
/// characters or a traversal segment. Same rule the OAuth `next` guard applies:
/// one definition of "local path" is enough, and this one is the realtime
/// peer.
fn local_route(raw: &str) -> Option<&str> {
    let route = raw.trim();
    if route.is_empty()
        || route.len() > MAX_COMMENT_ROUTE_LEN
        || !route.starts_with('/')
        || route.starts_with("//")
        || route
            .chars()
            .any(|c| c.is_control() || c == '\\' || c == ' ' || c == '#' || c == '?')
        || route
            .split('/')
            .any(|segment| segment == ".." || segment == ".")
    {
        return None;
    }
    Some(route)
}

/// Whether `channel` belongs to the caller's `user:<id>` namespace.
///
/// Identical to the SSE endpoint's rule: the namespace itself and its
/// colon-separated children pass, while `user:420` does not belong to user 42
/// and an `app:` channel is not the caller's to read.
fn channel_in_user_scope(channel: &str, user_id: &str) -> bool {
    let scope = format!("user:{user_id}");
    channel == scope
        || channel
            .strip_prefix(&scope)
            .is_some_and(|rest| rest.starts_with(':'))
}

/// Decide a `subscribe-comments` request: the room to join, or the error name
/// the socket answers with.
///
/// A comment stream is never public: the caller must be an authenticated
/// connection (RT-02), and the route must be a local path so a room name cannot
/// be an arbitrary string the client invented.
fn authorize_comment_room(
    connections: &HashMap<String, ConnectionAuth>,
    connection_id: &str,
    route: &str,
) -> Result<String, &'static str> {
    let route = local_route(route).ok_or("invalid-route")?;
    if !connections.contains_key(connection_id) {
        return Err("unauthenticated");
    }
    Ok(format!("comments:{route}"))
}

/// Decide a `subscribe-channel` request: the `channel:<c>` room to join, or the
/// error name the socket answers with.
///
/// This is the handler that made `io::publish` reach socket.io clients at all
/// (RT-03): without a socket in the room the fan-out was a silent no-op.
fn authorize_channel_room(
    connections: &HashMap<String, ConnectionAuth>,
    connection_id: &str,
    channel: &str,
) -> Result<String, &'static str> {
    let Some(auth) = connections.get(connection_id) else {
        return Err("unauthenticated");
    };
    let channel = channel.trim();
    if !channel_in_user_scope(channel, &auth.user_id) {
        return Err("forbidden");
    }
    Ok(format!("channel:{channel}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(user_id: &str) -> ConnectionAuth {
        ConnectionAuth {
            user_id: user_id.to_string(),
            user_name: user_id.to_string(),
            roles: vec!["User".to_string()],
            session_id: Some("session-1".to_string()),
            token_expires_at: None,
        }
    }

    fn connections(user_id: &str) -> HashMap<String, ConnectionAuth> {
        HashMap::from([("c1".to_string(), connection(user_id))])
    }

    /// RT-02: an anonymous socket may not subscribe to a comment stream, and
    /// the refusal happens before any room is created.
    #[test]
    fn an_anonymous_comment_subscription_is_refused() {
        let empty = HashMap::new();
        assert_eq!(
            authorize_comment_room(&empty, "c1", "/about"),
            Err("unauthenticated")
        );
    }

    /// RT-02: an authenticated socket still only gets a local route as a room.
    #[test]
    fn a_comment_route_must_be_local() {
        let conns = connections("ada@example.com");
        assert_eq!(
            authorize_comment_room(&conns, "c1", "/about").as_deref(),
            Ok("comments:/about")
        );
        for route in [
            "",
            "https://evil.example/x",
            "//evil.example/x",
            "/a/../b",
            "/a b",
            "/x?y=1",
            "relative",
        ] {
            assert_eq!(
                authorize_comment_room(&conns, "c1", route),
                Err("invalid-route"),
                "route {route:?} must be refused"
            );
        }
    }

    /// RT-03: a socket can join the `channel:<c>` room the fan-out targets, but
    /// only inside its own `user:` namespace — the same scope SSE enforces.
    #[test]
    fn a_socket_may_only_subscribe_to_its_own_channel() {
        let conns = connections("42");
        assert_eq!(
            authorize_channel_room(&conns, "c1", "user:42").as_deref(),
            Ok("channel:user:42")
        );
        assert_eq!(
            authorize_channel_room(&conns, "c1", "user:42:inbox").as_deref(),
            Ok("channel:user:42:inbox")
        );
        for channel in ["user:420", "app:test", ""] {
            assert_eq!(
                authorize_channel_room(&conns, "c1", channel),
                Err("forbidden"),
                "channel {channel:?} is outside the caller's scope"
            );
        }
        assert_eq!(
            authorize_channel_room(&HashMap::new(), "c1", "user:42"),
            Err("unauthenticated")
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocUnsubscription {
    pub doctype: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSubscription {
    pub doctype: String,
    pub filters: serde_json::Value,
    pub filters_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListUnsubscription {
    pub doctype: String,
    pub filters_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypingData {
    pub doctype: String,
    pub docname: String,
    pub field: Option<String>,
    /// Ignored on input: the server stamps the identity from the authenticated
    /// connection (RT4).
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub user_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopTypingData {
    pub doctype: String,
    pub docname: String,
    pub field: Option<String>,
    #[serde(default)]
    pub user_id: String,
}

/// Payload for the `set-status` socket event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetStatusData {
    pub status: crate::presence::UserStatus,
    pub message: Option<String>,
}

/// Payload for the `set-status-message` socket event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusMessageData {
    pub message: Option<String>,
}
