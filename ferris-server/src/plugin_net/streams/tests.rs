//! Address parsing and the boot-time listener binding.

use super::raw::split_authority;
use super::websocket::parse_ws_uri;

use super::*;
use ferris_wasm::descriptor::{NetworkSpec, RouteKind, RouteSpec};
use ferris_wasm::{RuntimeConfig, SharedRuntime, StreamListen, StreamTransport};

fn network(routes: Vec<RouteSpec>) -> NetworkSpec {
    NetworkSpec {
        capabilities: Vec::new(),
        routes,
        auth_mode: ferris_wasm::RouteAuth::None,
        http_policy: None,
        tls_client: None,
        tls_server: None,
    }
}

fn stream_route(id: &str, listener: Option<StreamTransport>) -> RouteSpec {
    RouteSpec {
        id: id.to_string(),
        method: "*".to_string(),
        path: format!("/{id}"),
        kind: RouteKind::Stream,
        auth: None,
        body_limit: None,
        tls_server: None,
        listener: listener.map(|transport| {
            StreamListen {
                transport,
                address: Some(format!("/{id}")),
                port: None,
                tls: None,
                auth: None,
                max_frame: None,
                idle_timeout_seconds: None,
            }
            .into()
        }),
    }
}

/// A transport over an empty store: `bind_declared_listeners` only reaches
/// the store for a listener it actually binds.
async fn transport() -> ServerStreams {
    let runtime = Arc::new(
        SharedRuntime::new(RuntimeConfig {
            enable_epoch_interrupts: false,
            ..Default::default()
        })
        .expect("SharedRuntime"),
    );
    let pool = ferris_db::memory_db().await.expect("in-memory sqlite pool");
    ServerStreams::new(Arc::new(PluginStore::new(runtime, pool)))
}

/// A declared WebSocket listener is an HTTP upgrade: the router mounts it, so
/// the boot phase must not report it as an unavailable socket.
#[tokio::test]
async fn a_declared_websocket_listener_is_left_to_the_router() {
    let transport = transport().await;
    let declarations = vec![(
        "app".to_string(),
        network(vec![stream_route("live", Some(StreamTransport::Websocket))]),
    )];
    let failures = bind_declared_listeners(&transport, &declarations).await;
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(transport.live(), 0, "no socket is bound for an upgrade");
}

/// A `%stream` route with no listener has no address the host could serve:
/// that is reported, never silently ignored.
#[tokio::test]
async fn a_stream_route_without_a_listener_is_reported() {
    let transport = transport().await;
    let declarations = vec![(
        "app".to_string(),
        network(vec![stream_route("orphan", None)]),
    )];
    let failures = bind_declared_listeners(&transport, &declarations).await;
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].1, "orphan");
    assert!(failures[0].2.contains("listener"), "{:?}", failures[0]);
}

#[test]
fn an_authority_is_split_with_or_without_a_scheme() {
    assert_eq!(
        split_authority("tcp://127.0.0.1:7000", Some("tcp://")).unwrap(),
        ("127.0.0.1".to_string(), 7000)
    );
    assert_eq!(
        split_authority("127.0.0.1:7000", None).unwrap(),
        ("127.0.0.1".to_string(), 7000)
    );
    assert_eq!(
        split_authority("tls://[::1]:7001", Some("tls://")).unwrap(),
        ("::1".to_string(), 7001)
    );
}

#[test]
fn a_missing_port_or_the_wrong_scheme_is_refused() {
    assert!(split_authority("tcp://127.0.0.1", Some("tcp://")).is_err());
    assert!(split_authority("ws://127.0.0.1:1", Some("tcp://")).is_err());
    assert!(split_authority("127.0.0.1:notaport", None).is_err());
    assert!(split_authority(":7000", None).is_err());
}

#[test]
fn a_websocket_uri_yields_the_authority_it_dials() {
    assert_eq!(
        parse_ws_uri("ws://127.0.0.1:9000/echo?x=1").unwrap(),
        ("127.0.0.1".to_string(), 9000, false)
    );
    assert_eq!(
        parse_ws_uri("wss://socket.example.test/live").unwrap(),
        ("socket.example.test".to_string(), 443, true)
    );
    assert_eq!(
        parse_ws_uri("ws://socket.example.test").unwrap(),
        ("socket.example.test".to_string(), 80, false)
    );
    assert_eq!(
        parse_ws_uri("wss://[::1]:9443/live").unwrap(),
        ("::1".to_string(), 9443, true)
    );
}

#[test]
fn a_non_websocket_uri_is_refused_instead_of_guessed() {
    for raw in ["tcp://127.0.0.1:9000", "https://host/", "127.0.0.1:9000"] {
        assert!(parse_ws_uri(raw).is_err(), "`{raw}` is not a websocket URI");
    }
    assert!(parse_ws_uri("ws://").is_err());
    assert!(parse_ws_uri("ws://host:notaport/").is_err());
}
