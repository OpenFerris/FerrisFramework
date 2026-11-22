//! Table validation for the routes apps declare: paths, modes and collisions.

use ferris_wasm::descriptor::{NetworkSpec, RouteKind, RouteSpec};
use ferris_wasm::{
    ListenerTransport, RouteAuth, StreamListen, StreamTransport, TlsMaterial, TlsServer,
};

use super::RouteMode;
use super::mount::{CAPABILITY_HEADER, constant_time_eq, tail};
use super::table::{RouteConflict, RouteTable, normalize_path};

fn network(routes: Vec<RouteSpec>) -> NetworkSpec {
    NetworkSpec {
        capabilities: Vec::new(),
        routes,
        auth_mode: RouteAuth::None,
        http_policy: None,
        tls_client: None,
        tls_server: None,
    }
}

fn spec(id: &str, method: &str, path: &str) -> RouteSpec {
    RouteSpec {
        id: id.to_string(),
        method: method.to_string(),
        path: path.to_string(),
        kind: RouteKind::Http,
        auth: None,
        body_limit: None,
        tls_server: None,
        listener: None,
    }
}

fn stream_spec(
    id: &str,
    path: &str,
    transport: StreamTransport,
    tls: Option<TlsServer>,
) -> RouteSpec {
    RouteSpec {
        id: id.to_string(),
        method: "*".to_string(),
        path: path.to_string(),
        kind: RouteKind::Stream,
        auth: None,
        body_limit: None,
        tls_server: tls,
        listener: Some(
            StreamListen {
                transport,
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

fn tls_record() -> TlsServer {
    TlsServer {
        cert: TlsMaterial::Pem("cert".to_string()),
        key: TlsMaterial::Pem("key".to_string()),
        ca: None,
        client_auth: ferris_wasm::ClientAuth::None,
        sni: Vec::new(),
        alpn: Vec::new(),
        min_version: ferris_wasm::TlsVersion::Tls12,
    }
}

/// A path under the framework's own surface is refused, never mounted: a plugin
/// that could claim `/api` would be able to answer the Desk's own requests.
#[test]
fn a_framework_prefix_is_refused() {
    let declared = vec![
        (
            "app".to_string(),
            network(vec![spec("a", "GET", "/api/probe")]),
        ),
        (
            "app".to_string(),
            network(vec![spec("b", "GET", "/plugins/app/x")]),
        ),
    ];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert!(table.routes.is_empty());
    assert_eq!(table.refused.len(), 2);
    assert!(matches!(
        table.refused[0].2,
        RouteConflict::FrameworkPrefix { .. }
    ));
}

/// `/` claims the whole namespace, which at the root is every path the framework
/// serves.
#[test]
fn the_root_path_is_refused() {
    let declared = vec![("app".to_string(), network(vec![spec("a", "GET", "/")]))];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert!(table.routes.is_empty());
    assert_eq!(table.refused[0].2, RouteConflict::RootClaim);
}

/// The host interprets no placeholders, so pattern syntax is refused instead of
/// being handed to axum as a wildcard.
#[test]
fn pattern_syntax_in_a_declared_path_is_refused() {
    for path in [
        "/files/{*rest}",
        "/x/*rest",
        "/x/:id",
        "/x?y=1",
        "/x/../api",
        "x",
    ] {
        let declared = vec![("app".to_string(), network(vec![spec("a", "GET", path)]))];
        let table = RouteTable::build_for(&declared, RouteMode::Plain);
        assert!(table.routes.is_empty(), "`{path}` must not be mountable");
        assert!(matches!(table.refused[0].2, RouteConflict::Invalid { .. }));
    }
}

/// Two apps declaring the same method on the same path cannot both be served:
/// the second is refused and the first keeps its route.
#[test]
fn a_cross_app_collision_refuses_the_second_app() {
    let declared = vec![
        (
            "alpha".to_string(),
            network(vec![spec("a", "GET", "/shared")]),
        ),
        (
            "beta".to_string(),
            network(vec![spec("b", "GET", "/shared")]),
        ),
    ];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert_eq!(table.routes.len(), 1);
    assert_eq!(table.routes[0].app, "alpha");
    assert!(matches!(
        table.refused[0].2,
        RouteConflict::Duplicate { .. }
    ));
}

/// Different methods on the same path are not a collision: axum merges them, and
/// so does the table.
#[test]
fn different_methods_of_one_path_coexist() {
    let declared = vec![
        (
            "alpha".to_string(),
            network(vec![spec("read", "GET", "/thing")]),
        ),
        (
            "beta".to_string(),
            network(vec![spec("write", "POST", "/thing")]),
        ),
    ];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert_eq!(table.routes.len(), 2);
    assert!(table.refused.is_empty());
}

/// A `*` route claims every method, so a later `GET` of the same path collides
/// with it.
#[test]
fn a_wildcard_method_claims_the_path_for_every_method() {
    let declared = vec![
        (
            "alpha".to_string(),
            network(vec![spec("all", "*", "/thing")]),
        ),
        (
            "alpha".to_string(),
            network(vec![spec("read", "GET", "/thing")]),
        ),
    ];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert_eq!(table.routes.len(), 1);
    assert!(matches!(
        table.refused[0].2,
        RouteConflict::Duplicate { ref method, .. } if method == "GET"
    ));
}

/// `GET` and `HEAD` share axum's slot, so declaring both is a collision the
/// table refuses instead of one the router panics on.
#[test]
fn head_and_get_of_one_path_collide() {
    let declared = vec![
        (
            "alpha".to_string(),
            network(vec![spec("read", "GET", "/thing")]),
        ),
        (
            "beta".to_string(),
            network(vec![spec("head", "HEAD", "/thing")]),
        ),
    ];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert_eq!(table.routes.len(), 1);
    assert!(matches!(
        table.refused[0].2,
        RouteConflict::Duplicate { ref method, .. } if method == "HEAD"
    ));
}

/// The mode decides which routes a table owns: the plain listener never serves a
/// `tls-server` route, and the TLS listener never serves a plain one.
#[test]
fn the_mode_selects_the_tls_surface() {
    let mut tls = spec("secure", "GET", "/secure");
    tls.tls_server = Some(tls_record());
    let declared = vec![(
        "app".to_string(),
        network(vec![spec("plain", "GET", "/plain"), tls]),
    )];

    let plain = RouteTable::build_for(&declared, RouteMode::Plain);
    assert_eq!(plain.routes.len(), 1);
    assert_eq!(plain.routes[0].id, "plain");

    let secure = RouteTable::build_for(&declared, RouteMode::Tls);
    assert_eq!(secure.routes.len(), 1);
    assert_eq!(secure.routes[0].id, "secure");
}

/// A route another surface owns is skipped silently: a `%stream` route and a
/// `tls-server` route are legitimate declarations, not conflicts, and logging
/// them as refused would train an operator to ignore the log.
#[test]
fn routes_owned_by_another_surface_are_not_conflicts() {
    let mut stream = spec("sensors", "*", "/sensors");
    stream.kind = RouteKind::Stream;
    let mut tls = spec("secure", "GET", "/secure");
    tls.tls_server = Some(tls_record());
    let declared = vec![(
        "app".to_string(),
        network(vec![stream, tls, spec("plain", "GET", "/plain")]),
    )];

    let plain = RouteTable::build_for(&declared, RouteMode::Plain);
    assert_eq!(plain.routes.len(), 1);
    assert_eq!(plain.routes[0].id, "plain");
    assert!(plain.refused.is_empty(), "{:?}", plain.refused);

    let secure = RouteTable::build_for(&declared, RouteMode::Tls);
    assert_eq!(secure.routes.len(), 1);
    assert_eq!(secure.routes[0].id, "secure");
    assert!(secure.refused.is_empty(), "{:?}", secure.refused);
}

/// A declared body limit travels verbatim; the global server limit is the only
/// ceiling, so the framework does not silently truncate what an app declared.
#[test]
fn the_body_limit_is_the_declared_value() {
    let mut small = spec("small", "POST", "/small");
    small.body_limit = Some(1024);
    let mut huge = spec("huge", "POST", "/huge");
    huge.body_limit = Some(u32::MAX);
    let declared = vec![("app".to_string(), network(vec![small, huge]))];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert_eq!(table.routes[0].body_limit, 1024);
    assert_eq!(
        table.routes[1].body_limit,
        usize::try_from(u32::MAX).expect("u32 fits usize")
    );
}

/// The guest sees the path after the matched prefix, never the whole URL.
#[test]
fn the_guest_path_is_the_tail_of_the_matched_prefix() {
    assert_eq!(tail("/probe", "/probe"), "/");
    assert_eq!(tail("/probe", "/probe/"), "/");
    assert_eq!(tail("/probe", "/probe/echo"), "/echo");
    assert_eq!(tail("/probe", "/probe/a/b"), "/a/b");
}

/// A route declared with no `auth` inherits the app default.
#[test]
fn a_route_without_auth_inherits_the_app_default() {
    let mut app = network(vec![spec("a", "GET", "/x")]);
    app.auth_mode = RouteAuth::Session;
    let declared = vec![("app".to_string(), app)];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert!(matches!(table.routes[0].auth, RouteAuth::Session));
}

/// Paths normalize to a canonical form before they are compared, so `/x/` and
/// `/x` are the same route and collide.
#[test]
fn paths_are_normalized_before_comparison() {
    assert_eq!(normalize_path("/x/").as_deref(), Some("/x"));
    assert_eq!(normalize_path("x").as_deref(), None);
    assert_eq!(normalize_path("/").as_deref(), Some("/"));
}

/// The capability comparison has no early exit and refuses a length mismatch.
#[test]
fn the_capability_token_comparison_is_constant_time_and_exact() {
    assert!(constant_time_eq(b"token", b"token"));
    assert!(!constant_time_eq(b"token", b"tokeN"));
    assert!(!constant_time_eq(b"token", b"token "));
    assert!(!constant_time_eq(b"", b"x"));
}

/// The header name is part of the contract the guest never sees.
#[test]
fn the_capability_header_is_the_documented_one() {
    assert_eq!(CAPABILITY_HEADER, "x-ferris-capability");
}

/// A `%stream` route whose listener is a WebSocket upgrade is mounted by this
/// table: it is an HTTP handshake, not a socket the boot phase binds. It is
/// claimed as a `GET` (the only method an upgrade has) whatever the declaration
/// says.
#[test]
fn a_websocket_stream_route_is_mounted_as_a_get() {
    let mut route = stream_spec("live", "/live", StreamTransport::Websocket, None);
    route.method = "POST".to_string();
    let declared = vec![("app".to_string(), network(vec![route]))];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert_eq!(table.routes.len(), 1, "{:?}", table.refused);
    assert!(table.routes[0].stream);
    assert_eq!(table.routes[0].method, "GET");
    assert!(table.refused.is_empty());
}

/// A stream route claims exactly its path: the frames carry no path, so a
/// subtree would accept traffic the app never declared.
#[test]
fn a_stream_route_claims_exactly_its_path() {
    let declared = vec![(
        "app".to_string(),
        network(vec![stream_spec(
            "live",
            "/live",
            StreamTransport::Websocket,
            None,
        )]),
    )];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert_eq!(table.routes[0].patterns(), vec!["/live".to_string()]);
}

/// `wss` is an upgrade over TLS, so it belongs to the app's dedicated TLS
/// listener and never to the plain one.
#[test]
fn a_wss_stream_route_belongs_to_the_tls_listener() {
    let declared = vec![(
        "app".to_string(),
        network(vec![stream_spec(
            "live",
            "/live",
            StreamTransport::Websocket,
            Some(tls_record()),
        )]),
    )];
    let plain = RouteTable::build_for(&declared, RouteMode::Plain);
    assert!(
        plain.routes.is_empty(),
        "a wss route is not served in clear text"
    );
    assert!(
        plain.refused.is_empty(),
        "it is another surface's route, not a conflict: {:?}",
        plain.refused
    );
    let secure = RouteTable::build_for(&declared, RouteMode::Tls);
    assert_eq!(secure.routes.len(), 1);
    assert!(secure.routes[0].stream);
}

/// An upgrade and a plain `GET` of the same path cannot share axum's slot: the
/// table refuses the second instead of panicking at mount time.
#[test]
fn an_upgrade_collides_with_a_get_of_the_same_path() {
    let declared = vec![
        (
            "alpha".to_string(),
            network(vec![stream_spec(
                "live",
                "/live",
                StreamTransport::Websocket,
                None,
            )]),
        ),
        (
            "beta".to_string(),
            network(vec![spec("read", "GET", "/live")]),
        ),
    ];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert_eq!(table.routes.len(), 1);
    assert_eq!(table.routes[0].app, "alpha");
    assert!(
        matches!(&table.refused[0].2, RouteConflict::Duplicate { method, .. } if method == "GET"),
        "{:?}",
        table.refused
    );
}

/// A `tcp`/`tls` listener is a raw socket the boot phase binds, not a route any
/// HTTP router carries — and that is not a conflict either.
#[test]
fn a_raw_stream_listener_is_not_mounted_by_a_router() {
    for transport in [StreamTransport::Tcp, StreamTransport::Tls] {
        let declared = vec![(
            "app".to_string(),
            network(vec![stream_spec("sensors", "/sensors", transport, None)]),
        )];
        for mode in [RouteMode::Plain, RouteMode::Tls] {
            let table = RouteTable::build_for(&declared, mode);
            assert!(table.routes.is_empty(), "{transport:?} {mode:?}");
            assert!(table.refused.is_empty(), "{:?}", table.refused);
        }
    }
}

/// A `%stream` route with no listener has no address at all: it is not mounted
/// and not reported here (the boot phase reports it when it looks for a socket).
#[test]
fn a_stream_route_without_a_listener_is_not_mounted() {
    let mut route = spec("orphan", "GET", "/orphan");
    route.kind = RouteKind::Stream;
    let declared = vec![("app".to_string(), network(vec![route]))];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert!(table.routes.is_empty());
    assert!(table.refused.is_empty());
}

/// The declared `max-frame` and idle timeout travel into the upgrade context.
#[test]
fn a_stream_route_carries_its_listener_limits() {
    let mut route = stream_spec("live", "/live", StreamTransport::Websocket, None);
    if let Some(listener) = route.listener.as_mut() {
        listener.max_frame = Some(4096);
        listener.idle_timeout_seconds = Some(30);
    }
    let declared = vec![("app".to_string(), network(vec![route]))];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert_eq!(table.routes[0].max_frame(), Some(4096));
    assert_eq!(
        table.routes[0].idle(),
        Some(std::time::Duration::from_secs(30))
    );
}

/// The listener the app declared is what the table mounts: a regression that
/// stopped recognising `websocket` would silently unmount every declared
/// upgrade.
#[test]
fn a_stream_route_keeps_the_transport_it_declared() {
    let declared = vec![(
        "app".to_string(),
        network(vec![stream_spec(
            "live",
            "/live",
            StreamTransport::Websocket,
            None,
        )]),
    )];
    let table = RouteTable::build_for(&declared, RouteMode::Plain);
    assert_eq!(
        table.routes[0].listener.as_ref().map(|l| l.transport),
        Some(ListenerTransport::Websocket)
    );
    assert!(table.routes[0].max_frame().is_none());
    assert!(table.routes[0].idle().is_none());
}
