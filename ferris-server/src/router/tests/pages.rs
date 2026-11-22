use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{StatusCode, header},
    middleware::from_fn,
    routing::get,
};
use ferris_core::ui_extensions::{AppUiSpec, DashboardSpec, register_app_ui};
use tower::ServiceExt;

use super::super::pages::{live_page_guard, page_path_matches};

fn dashboard(id: &str, route: &str) -> DashboardSpec {
    DashboardSpec {
        id: id.into(),
        label: id.into(),
        route: route.into(),
        ..Default::default()
    }
}

/// Fase 3 / riesgo R2: the guard serves what the LIVE registry declares
/// — including a route registered after the router existed, the dev
/// hot-reload case the guard exists for — and delegates everything else
/// to the same `not_found_fallback` as before. A bare `/{*rest}` would
/// have SSR'd the desk shell for every one of those 404s.
#[tokio::test]
async fn the_guard_serves_live_pages_and_falls_back_everywhere_else() {
    // Registered AFTER the "router" below is built: the guard asks the
    // live registry per request, so the order does not matter.
    let registration = register_app_ui(
        "guard-test",
        AppUiSpec {
            dashboards: vec![dashboard("late", "/guard-test-late")],
            ..Default::default()
        },
    );
    assert!(
        registration.warnings.is_empty(),
        "unexpected warnings: {:?}",
        registration.warnings
    );

    let app: Router = Router::new()
        .route("/{*rest}", get(|| async { "shell" }))
        .route_layer(from_fn(live_page_guard));

    // Declared (late) → the catch-all handler runs.
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/guard-test-late")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Undeclared → the 404 the fallback has always given (HTML page for
    // browsers, which is what a navigation without an API Accept is).
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/nope").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // API-shaped callers keep the JSON error body — the swallow this
    // guard exists to prevent.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/unknown")
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "not_found");
}

/// The pattern rule the guard asks, mirroring `axum_page_path`'s
/// normalization: `:param` one segment, `*rest` the remainder, literals
/// only themselves, empty segments collapsed on both sides.
#[test]
fn declared_patterns_match_request_paths() {
    assert!(page_path_matches("/status", "/status"));
    assert!(page_path_matches("/status", "/status/"));
    assert!(page_path_matches("/blog/:slug", "/blog/hola"));
    assert!(!page_path_matches("/blog/:slug", "/blog/hola/mas"));
    assert!(!page_path_matches("/blog/:slug", "/blog"));
    assert!(page_path_matches("/docs/*rest", "/docs/a/b"));
    assert!(page_path_matches("/docs/*rest", "/docs"));
    assert!(!page_path_matches("/docs/*rest", "/other"));
    assert!(page_path_matches("/", "/"));
    assert!(!page_path_matches("/", "/status"));
}
