//! The `%stream` routes an app declares: mounted at their path, gated by their
//! own auth, and closed cleanly.

use std::sync::Arc;

use axum::Router;
use ferris_server::router::plugin_routes::{RouteMode, declared_route_router};
use ferris_wasm::descriptor::{NetworkSpec, RouteKind, RouteSpec};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::support::*;

/// A declared `%stream` route whose listener is a WebSocket upgrade.
pub fn ws_route(id: &str, path: &str, auth: Option<RouteAuth>) -> RouteSpec {
    RouteSpec {
        id: id.to_string(),
        method: "GET".to_string(),
        path: path.to_string(),
        kind: RouteKind::Stream,
        auth,
        body_limit: None,
        tls_server: None,
        listener: Some(
            StreamListen {
                transport: StreamTransport::Websocket,
                address: Some(path.to_string()),
                port: None,
                tls: None,
                auth: None,
                max_frame: None,
                idle_timeout_seconds: None,
            }
            .into(),
        ),
    }
}

/// Serve `router` on a loopback port, the way `build_router` is served.
async fn serve(router: Router) -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the router port binds");
    let port = listener.local_addr().expect("address").port();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (port, handle)
}

// -- declared `%stream` routes ------------------------------------------------

/// A declared `%stream` route is mounted at its path as an upgrade: a real
/// client completes the handshake, a frame crosses to `handle-stream` and the
/// guest's reply comes back, and the close is the protocol's, not a dropped
/// socket.
#[tokio::test]
async fn a_declared_stream_route_upgrades_and_echoes() {
    let Some(store) = store().await else { return };
    let declarations = vec![(
        "probe".to_string(),
        NetworkSpec {
            capabilities: Vec::new(),
            routes: vec![ws_route("live", "/live", None)],
            auth_mode: RouteAuth::None,
            http_policy: None,
            tls_client: None,
            tls_server: None,
        },
    )];
    let router = declared_route_router(Some(store), auth(), &declarations, RouteMode::Plain);
    let (port, server) = serve(router).await;

    let (mut ws, response) = tokio::time::timeout(
        TIMEOUT,
        connect_async(format!("ws://127.0.0.1:{port}/live")),
    )
    .await
    .expect("the upgrade completes in time")
    .expect("the declared route accepts the upgrade");
    assert_eq!(response.status().as_u16(), 101);

    ws.send(Message::text("ping")).await.expect("frame sent");
    let reply = tokio::time::timeout(TIMEOUT, ws.next())
        .await
        .expect("the reply arrives in time")
        .expect("a frame arrives")
        .expect("the frame is valid");
    // The guest's reply is bytes, so the host relays one binary frame: the
    // WIT's `handle-stream` has no text/binary distinction to preserve.
    assert_eq!(reply, Message::binary(b"ping".to_vec()));

    // A clean close: the host answers the Close frame instead of dropping the
    // TCP connection.
    ws.close(None).await.expect("the close frame is sent");
    let closed = tokio::time::timeout(TIMEOUT, ws.next())
        .await
        .expect("the close arrives in time");
    assert!(
        matches!(closed, Some(Ok(Message::Close(_)))),
        "expected a Close frame, got {closed:?}"
    );
    server.abort();
}

/// A `%stream` route declaring `auth: session` refuses the upgrade itself: the
/// client sees a `401` handshake, never an open stream.
#[tokio::test]
async fn a_declared_stream_route_refuses_an_anonymous_upgrade() {
    let Some(store) = store().await else { return };
    let declarations = vec![(
        "probe".to_string(),
        NetworkSpec {
            capabilities: Vec::new(),
            routes: vec![ws_route("private", "/private", Some(RouteAuth::Session))],
            auth_mode: RouteAuth::None,
            http_policy: None,
            tls_client: None,
            tls_server: None,
        },
    )];
    let router = declared_route_router(Some(store), auth(), &declarations, RouteMode::Plain);
    let (port, server) = serve(router).await;

    match connect_async(format!("ws://127.0.0.1:{port}/private")).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status(), 401, "a session route gates the upgrade");
        }
        other => panic!("expected a refused handshake, got {other:?}"),
    }
    server.abort();
}

/// A `%stream` route declaring a capability answers `403` until the app
/// provisions the token and `101` once the header matches — the same gate the
/// request path uses.
#[tokio::test]
async fn a_declared_stream_route_enforces_its_capability_token() {
    let Some(store) = store().await else { return };
    let declarations = vec![(
        "probe".to_string(),
        NetworkSpec {
            capabilities: Vec::new(),
            routes: vec![ws_route(
                "machine",
                "/machine",
                Some(RouteAuth::Capability("machine".to_string())),
            )],
            auth_mode: RouteAuth::None,
            http_policy: None,
            tls_client: None,
            tls_server: None,
        },
    )];
    let router = declared_route_router(
        Some(Arc::clone(&store)),
        auth(),
        &declarations,
        RouteMode::Plain,
    );
    let (port, server) = serve(router).await;

    let url = format!("ws://127.0.0.1:{port}/machine");
    match connect_async(url.clone()).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(
                response.status(),
                403,
                "no provisioned token is a 403, not an open stream"
            );
        }
        other => panic!("expected a refused handshake, got {other:?}"),
    }

    let pool = store.pool();
    let upsert = ferris_db::sql::UpsertBuilder::new(pool.dialect(), "tabAppConfig")
        .values(vec![
            ("plugin", ferris_core::Value::from("probe")),
            ("key", ferris_core::Value::from("route.machine.token")),
            ("value", ferris_core::Value::from("s3cret")),
        ])
        .conflict_on(["plugin", "key"])
        .on_update(
            "value",
            ferris_db::sql::UpsertSet::Value(ferris_core::Value::from("s3cret")),
        )
        .build()
        .expect("upsert builds");
    pool.upsert(upsert).await.expect("the token row is stored");

    let mut request = url.into_client_request().expect("the ws request builds");
    request
        .headers_mut()
        .insert("x-ferris-capability", "s3cret".parse().expect("header"));
    let (mut ws, response) = connect_async(request)
        .await
        .expect("the matching token upgrades");
    assert_eq!(response.status().as_u16(), 101);
    ws.send(Message::binary(b"machine frame".to_vec()))
        .await
        .expect("frame sent");
    let reply = tokio::time::timeout(TIMEOUT, ws.next())
        .await
        .expect("the reply arrives in time")
        .expect("a frame arrives")
        .expect("the frame is valid");
    assert_eq!(reply, Message::binary(b"machine frame".to_vec()));
    server.abort();
}
