//! End-to-end coverage of the routes an app declares in `describe()`.
//!
//! The guest is `probe-ok`: it declares one catch-all route and answers
//! `/echo`, `/teapot` and the rest by parsing `http-request.path` itself, which
//! is the split the contract makes (the host matches the prefix, the guest owns
//! the rest). The tests mount hand-built route specs pointing at that guest, so
//! what is under test is the *host* half: path matching, the auth gate, the body
//! cap, collision refusal and the `route-id` the guest receives.
//!
//! Build the fixture first (the tests skip loudly when it is absent):
//! ```sh
//! cargo build --target wasm32-wasip2 \
//!   --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml
//! ```

#![allow(unused_crate_dependencies)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ferris_auth::{AuthState, SessionConfig};
use ferris_server::router::plugin_routes::{RouteMode, declared_route_router};
use ferris_wasm::descriptor::{NetworkSpec, RouteKind, RouteSpec};
use ferris_wasm::{PluginStore, RuntimeConfig, SharedRuntime};
use tower::ServiceExt;

fn probe_ok() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ferris-wasm/tests/fixtures/probe-ok/target/wasm32-wasip2/debug/probe_ok.wasm")
}

/// A store with `probe-ok` published as `probe`, or `None` when the fixture was
/// never built (a fresh checkout — say so instead of failing).
async fn store() -> Option<Arc<PluginStore>> {
    let path = probe_ok();
    if !path.exists() {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 \
             --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml"
        );
        return None;
    }
    let runtime = Arc::new(
        SharedRuntime::new(RuntimeConfig {
            enable_epoch_interrupts: false,
            ..Default::default()
        })
        .expect("SharedRuntime"),
    );
    // One connection: `:memory:` gives every pooled connection its own empty
    // database, so the host's config table would not be visible to the router.
    let pool = ferris_db::memory_db().await.expect("in-memory sqlite pool");
    // The guest declares `routes` + `docstore-write`; grant them the way an
    // operator would (internals are always there).
    let store = Arc::new(PluginStore::new(runtime, pool));
    store
        .swap_load_with_name(&path, "probe")
        .expect("probe-ok instantiates");
    // `describe()` is what installs the grant and the network surface.
    store.describe_of("probe").expect("probe describes itself");
    Some(store)
}

fn auth() -> AuthState {
    AuthState::new(SessionConfig::default())
}

fn spec(id: &str, method: &str, path: &str, auth: Option<ferris_wasm::RouteAuth>) -> RouteSpec {
    RouteSpec {
        id: id.to_string(),
        method: method.to_string(),
        path: path.to_string(),
        kind: RouteKind::Http,
        auth,
        body_limit: None,
        tls_server: None,
        listener: None,
    }
}

#[allow(clippy::missing_const_for_fn)] // it builds owned values; kept symmetric with `spec`
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

/// The declared path is a prefix: the guest receives only the tail, under the
/// `route-id` the host matched, and its response passes back verbatim.
#[tokio::test]
async fn a_declared_route_dispatches_with_its_id_and_the_path_tail() {
    let Some(store) = store().await else { return };
    let declarations = vec![(
        "probe".to_string(),
        network(vec![spec("probe", "*", "/probe", None)]),
    )];
    let router = declared_route_router(Some(store), auth(), &declarations, RouteMode::Plain);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/probe/echo?x=1")
                .header("x-probe-key", "yes")
                .body(Body::from("body-bytes"))
                .expect("request builds"),
        )
        .await
        .expect("router answers");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-probe")
            .and_then(|v| v.to_str().ok()),
        Some("echo"),
        "the guest's own headers survive"
    );
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body reads");
    assert_eq!(
        String::from_utf8_lossy(&body),
        "GET|x=1|yes|body-bytes",
        "method, query, header and body reach the guest, and the tail is `/echo`"
    );
}

/// A `session` route is a `401` before dispatch when the caller is anonymous.
#[tokio::test]
async fn a_session_route_refuses_an_anonymous_caller() {
    let Some(store) = store().await else { return };
    let declarations = vec![(
        "probe".to_string(),
        network(vec![spec(
            "private",
            "GET",
            "/private",
            Some(ferris_wasm::RouteAuth::Session),
        )]),
    )];
    let router = declared_route_router(Some(store), auth(), &declarations, RouteMode::Plain);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/private/echo")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// A `capability` route is `403` when the app never provisioned its token and
/// `401` for a wrong one, so an operator can tell the two apart.
#[tokio::test]
async fn a_capability_route_needs_a_provisioned_matching_token() {
    let Some(store) = store().await else { return };
    let declarations = vec![(
        "probe".to_string(),
        network(vec![spec(
            "machine",
            "POST",
            "/machine",
            Some(ferris_wasm::RouteAuth::Capability("machine".to_string())),
        )]),
    )];
    let router = declared_route_router(
        Some(Arc::clone(&store)),
        auth(),
        &declarations,
        RouteMode::Plain,
    );

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/machine/echo")
                .header("x-ferris-capability", "secret")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "no `route.machine.token` row: the app never provisioned it"
    );

    // Provision the token the way `config-set` would.
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

    let wrong = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/machine/echo")
                .header("x-ferris-capability", "nope")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(
        wrong.status(),
        StatusCode::UNAUTHORIZED,
        "a wrong token is a 401, never a dispatch"
    );

    let right = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/machine/echo")
                .header("x-ferris-capability", "s3cret")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(
        right.status(),
        StatusCode::OK,
        "the matching token dispatches to the guest"
    );
}

/// A path the framework owns is refused: the router does not answer it, and
/// `/api/**` keeps belonging to the framework.
#[tokio::test]
async fn a_route_on_a_framework_prefix_is_not_mounted() {
    let Some(store) = store().await else { return };
    let declarations = vec![(
        "probe".to_string(),
        network(vec![spec("shadow", "GET", "/api/shadow", None)]),
    )];
    let router = declared_route_router(Some(store), auth(), &declarations, RouteMode::Plain);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/shadow")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "the shadow route must not exist"
    );
}

/// The declared body limit is enforced before the guest runs.
#[tokio::test]
async fn a_declared_body_limit_is_enforced_at_the_edge() {
    let Some(store) = store().await else { return };
    let mut limited = spec("limited", "POST", "/limited", None);
    limited.body_limit = Some(16);
    let declarations = vec![("probe".to_string(), network(vec![limited]))];
    let router = declared_route_router(Some(store), auth(), &declarations, RouteMode::Plain);

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/limited/echo")
                .body(Body::from(vec![b'x'; 64]))
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
