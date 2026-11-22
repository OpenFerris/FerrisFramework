//! Server-Sent Events bridge for plugin `pubsub` channels.
//!
//! The realtime role fans `pubsub::publish` out over socket.io, but the Desk
//! ships no socket.io client, so a plugin widget cannot receive push updates
//! over that transport. This module adds a dependency-light second fan-out:
//!
//! * [`PubsubBus`] is a process-wide broadcast channel.
//! * [`FanoutBroadcaster`] implements [`RealtimeBroadcaster`]: it feeds the bus
//!   **and** mirrors the publish onto socket.io when the realtime role is
//!   mounted, so a channel reaches SSE and socket.io subscribers alike.
//! * `GET /events?channel=<c>` is a `text/event-stream` endpoint a widget opens
//!   with `EventSource`; each published message arrives as an SSE event named
//!   after its `event_type`, with the plugin's payload as its data.
//!
//! Channels are the same opaque names apps pass to `pubsub::publish`
//! (`"app:test"`), but a name is **not** a capability: `/events` requires an
//! authenticated principal and only subscribes to channels inside that
//! principal's namespace (`user:{id}` or `user:{id}:…`), matching the
//! authenticated socket.io transport this bus mirrors (SRV1). An app that wants
//! to push to one user publishes on that user's namespace.

use std::convert::Infallible;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use ferris_auth::middleware::OptionalUser;
#[cfg(feature = "realtime")]
use ferris_core::Value;
#[cfg(feature = "realtime")]
use ferris_realtime::{EventPayload, EventTarget, EventType, RealtimeEvent};
use ferris_wasm::RealtimeBroadcaster;
use serde::Deserialize;
#[cfg(feature = "realtime")]
use socketioxide::SocketIo;
use tokio::sync::broadcast;
use tokio_stream::{Stream, StreamExt as _, wrappers::BroadcastStream};

/// One published pubsub message. The payload is the plugin's raw JSON string,
/// carried verbatim so the browser can decide how to parse it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PubsubEvent {
    /// Channel the message was published on (`"app:test"`).
    pub channel: String,
    /// Event name the SSE frame carries (`"tasks_changed"`).
    pub event_type: String,
    /// Opaque payload string the plugin published.
    pub payload: String,
}

/// Process-wide pubsub bus. The broadcaster publishes into it and the SSE
/// endpoint subscribes; it is independent of socket.io so a server without the
/// realtime role still delivers plugin events to browser widgets.
#[derive(Clone)]
pub struct PubsubBus {
    tx: broadcast::Sender<PubsubEvent>,
}

impl PubsubBus {
    /// A new bus. The bounded buffer drops the oldest messages for a consumer
    /// that falls behind (broadcast semantics) instead of stalling publishers.
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(256);
        Self { tx }
    }

    /// Publish to every current subscriber. Fails only when nobody is
    /// subscribed, which is not an error at the transport edge.
    fn send(&self, event: PubsubEvent) {
        let _ = self.tx.send(event);
    }

    /// A receiver for one SSE connection.
    fn subscribe(&self) -> broadcast::Receiver<PubsubEvent> {
        self.tx.subscribe()
    }
}

impl Default for PubsubBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Pubsub fan-out: the SSE bus always, socket.io when the realtime role is
/// mounted. Registered once per process through
/// [`ferris_wasm::set_realtime_broadcaster`].
///
/// The socket.io half is a compile-time field: a build with `wasm` but no
/// `realtime` still delivers plugin events to browser widgets over SSE, which is
/// the whole reason the bridge exists.
pub struct FanoutBroadcaster {
    bus: PubsubBus,
    #[cfg(feature = "realtime")]
    socket_io: Option<SocketIo>,
}

impl FanoutBroadcaster {
    /// `socket_io` is `None` on a server without the realtime role: publishes
    /// still reach SSE subscribers.
    #[cfg(feature = "realtime")]
    pub fn new(bus: PubsubBus, socket_io: Option<SocketIo>) -> Self {
        Self { bus, socket_io }
    }

    /// The SSE-only constructor used when the realtime engine is not linked.
    #[cfg(not(feature = "realtime"))]
    pub fn new(bus: PubsubBus) -> Self {
        Self { bus }
    }
}

impl RealtimeBroadcaster for FanoutBroadcaster {
    fn publish(&self, channel: &str, event_type: &str, payload: &str) -> anyhow::Result<()> {
        // SSE first: this never fails and works without the realtime role.
        self.bus.send(PubsubEvent {
            channel: channel.to_string(),
            event_type: event_type.to_string(),
            payload: payload.to_string(),
        });

        // Mirror onto socket.io with the same envelope the realtime engine
        // emits, so a future socket.io client sees an identical message.
        #[cfg(feature = "realtime")]
        if let Some(io) = &self.socket_io {
            let event_type = EventType::from(event_type.to_string());
            let data = serde_json::from_str(payload)
                .map(Value::from_json)
                .unwrap_or(Value::Null);
            let event = RealtimeEvent::new(
                event_type.clone(),
                EventPayload::new(event_type, data),
                EventTarget::Room(format!("channel:{channel}")),
            );
            let _ = io
                .to(format!("channel:{channel}"))
                .emit("realtime", serde_json::to_value(event)?);
        }

        Ok(())
    }
}

/// Query for the SSE endpoint: the channel to subscribe to.
#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    /// Channel name, identical to the one passed to `pubsub::publish`.
    pub channel: String,
}

/// `GET /events?channel=<c>` — the live feed a plugin widget opens with
/// `EventSource`. Each matching publish becomes an SSE frame named after its
/// `event_type`; a periodic keep-alive comment keeps the connection warm.
///
/// Requires a session and refuses to subscribe to a channel outside the
/// caller's `user:{id}` namespace (SRV1): the pubsub payload is raw plugin JSON,
/// so a channel name cannot be the only thing protecting it.
pub async fn sse_events(
    State(bus): State<PubsubBus>,
    OptionalUser(user): OptionalUser,
    Query(query): Query<EventsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let Some(user) = user else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    let scope = format!("user:{}", user.id);
    if !channel_in_scope(&query.channel, &scope) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(Sse::new(event_stream(bus, query.channel)).keep_alive(KeepAlive::default()))
}

/// Whether `channel` belongs to `scope` — the caller's `user:{id}` namespace.
///
/// The namespace itself and its colon-separated children are allowed, so
/// `user:42` and `user:42:inbox` both pass while `user:420` and `app:test` do
/// not (`app:test` must instead be published as `user:42:app:test` to reach
/// user 42). This is what stops a channel name from acting as a capability.
fn channel_in_scope(channel: &str, scope: &str) -> bool {
    channel == scope
        || channel
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with(':'))
}

/// The filtered event stream behind [`sse_events`], split out so it can be
/// tested without an HTTP round trip.
fn event_stream(bus: PubsubBus, channel: String) -> impl Stream<Item = Result<Event, Infallible>> {
    let events = BroadcastStream::new(bus.subscribe()).filter_map(move |item| {
        let event = item.ok()?;
        (event.channel == channel)
            .then(|| Ok(Event::default().event(event.event_type).data(event.payload)))
    });
    // An open `EventSource` is a live connection for the whole browser session.
    // End it when the process starts draining so `axum` is not left waiting for
    // a client that has no reason to disconnect.
    ferris_api::shutdown::stop_on_shutdown(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::extract::FromRef;
    use axum::http::Request as HttpRequest;
    use axum::http::header;
    use axum::routing::get;
    use ferris_auth::{AuthState, JwtConfig, JwtManager, SessionConfig, SessionManager};
    use tower::ServiceExt;

    /// Minimal state so the handler can be exercised through a real router:
    /// `OptionalUser` only needs `AuthState`, and the handler needs `PubsubBus`.
    #[derive(Clone)]
    struct SseTestState {
        auth: AuthState,
        bus: PubsubBus,
    }

    impl FromRef<SseTestState> for AuthState {
        fn from_ref(state: &SseTestState) -> Self {
            state.auth.clone()
        }
    }

    impl FromRef<SseTestState> for PubsubBus {
        fn from_ref(state: &SseTestState) -> Self {
            state.bus.clone()
        }
    }

    fn state() -> SseTestState {
        let jwt = JwtManager::new(
            JwtConfig::new("sse-test-secret-at-least-32-bytes-long").expect("valid test secret"),
        )
        .expect("jwt manager");
        let auth = AuthState::new(
            jwt,
            SessionManager::new(SessionConfig::default()),
            SessionConfig::default(),
        );
        SseTestState {
            auth,
            bus: PubsubBus::new(),
        }
    }

    fn app(state: SseTestState) -> Router {
        Router::new()
            .route("/events", get(sse_events))
            .with_state(state)
    }

    /// The SSE-only broadcaster: the constructor's shape depends on whether the
    /// socket.io engine was compiled, not on the test.
    fn broadcaster(bus: PubsubBus) -> FanoutBroadcaster {
        #[cfg(feature = "realtime")]
        {
            FanoutBroadcaster::new(bus, None)
        }
        #[cfg(not(feature = "realtime"))]
        {
            FanoutBroadcaster::new(bus)
        }
    }

    fn request(uri: &str, token: Option<&str>) -> HttpRequest<Body> {
        let builder = HttpRequest::builder().uri(uri);
        let builder = match token {
            Some(token) => builder.header(header::AUTHORIZATION, format!("Bearer {token}")),
            None => builder,
        };
        builder.body(Body::empty()).expect("request builds")
    }

    #[tokio::test]
    async fn channel_scope_accepts_only_the_callers_namespace() {
        assert!(channel_in_scope("user:42", "user:42"));
        assert!(channel_in_scope("user:42:inbox", "user:42"));
        assert!(!channel_in_scope("user:420", "user:42"));
        assert!(!channel_in_scope("app:test", "user:42"));
        assert!(!channel_in_scope("", "user:42"));
    }

    #[tokio::test]
    async fn events_requires_authentication() {
        let response = app(state())
            .oneshot(request("/events?channel=user:42", None))
            .await
            .expect("the router answers");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn events_refuses_a_channel_outside_the_callers_scope() {
        let state = state();
        let token = state
            .auth
            .jwt_manager
            .generate_tokens("42", "user@example.com", &[], None, "session-1")
            .expect("token pair")
            .access_token;
        let response = app(state)
            .oneshot(request("/events?channel=app:test", Some(&token)))
            .await
            .expect("the router answers");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn events_accepts_a_channel_inside_the_callers_scope() {
        let state = state();
        let token = state
            .auth
            .jwt_manager
            .generate_tokens("42", "user@example.com", &[], None, "session-1")
            .expect("token pair")
            .access_token;
        let response = app(state)
            .oneshot(request("/events?channel=user:42:inbox", Some(&token)))
            .await
            .expect("the router answers");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn fanout_broadcaster_publishes_to_the_bus_without_socket_io() {
        let bus = PubsubBus::new();
        let mut rx = bus.subscribe();
        let broadcaster = broadcaster(bus);

        broadcaster
            .publish("app:test", "tasks_changed", r#"{"count":1}"#)
            .expect("publishing to the bus cannot fail");

        let event = rx.recv().await.expect("the subscriber receives the event");
        assert_eq!(event.channel, "app:test");
        assert_eq!(event.event_type, "tasks_changed");
        assert_eq!(event.payload, r#"{"count":1}"#);
    }

    #[tokio::test]
    async fn publishing_without_subscribers_is_not_an_error() {
        let broadcaster = broadcaster(PubsubBus::new());
        assert!(broadcaster.publish("app:test", "noop", "{}").is_ok());
    }

    #[tokio::test]
    async fn event_stream_only_emits_the_requested_channel() {
        let bus = PubsubBus::new();
        // Subscribe first: `event_stream` subscribes on creation, then the
        // broadcast bus delivers everything published afterwards.
        let mut stream = Box::pin(event_stream(bus.clone(), "app:test".to_string()));

        bus.send(PubsubEvent {
            channel: "other".to_string(),
            event_type: "ignored".to_string(),
            payload: "{}".to_string(),
        });
        bus.send(PubsubEvent {
            channel: "app:test".to_string(),
            event_type: "tasks_changed".to_string(),
            payload: r#"{"count":2}"#.to_string(),
        });

        let event = stream
            .next()
            .await
            .expect("one matching event")
            .expect("infallible");
        let debug = format!("{event:?}");
        assert!(debug.contains("tasks_changed"), "{debug}");
        assert!(debug.contains(r#"{\"count\":2}"#), "{debug}");
    }
}
