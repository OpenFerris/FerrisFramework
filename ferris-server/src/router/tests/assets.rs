use axum::{
    Router,
    body::Body,
    extract::{Path as AxumPath, Request},
    http::{StatusCode, header},
    middleware::from_fn,
    routing::get,
};
use tower::ServiceExt;

use super::super::assets::{pkg_no_store, preview_asset, vendor_asset};
use super::super::security::security_headers;

/// T2-F4: `/static/vendor/*` serves the embedded table with the right
/// `Content-Type` and cache, and answers 404 for anything uncommitted —
/// a traversal-shaped path included, because the lookup compares against
/// fixed keys and never touches the filesystem.
#[tokio::test]
async fn serves_committed_files_and_404s_the_rest() {
    let ok = vendor_asset(AxumPath("leaflet/leaflet.js".into())).await;
    assert_eq!(ok.status(), StatusCode::OK);
    assert!(
        ok.headers()
            .get(header::CONTENT_TYPE)
            .expect("content-type")
            .to_str()
            .expect("ascii")
            .starts_with("application/javascript")
    );
    assert_eq!(
        ok.headers().get(header::CACHE_CONTROL).expect("cache"),
        "public, max-age=86400"
    );

    let missing = vendor_asset(AxumPath("leaflet/nope.js".into())).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let traversal = vendor_asset(AxumPath("../../Cargo.toml".into())).await;
    assert_eq!(traversal.status(), StatusCode::NOT_FOUND);

    // The route itself is registered where the auth guard lets assets
    // through — prove a sessionless request is not bounced to /login.
    let app = Router::new()
        .route("/static/vendor/{*path}", get(vendor_asset))
        .layer(from_fn(security_headers));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/static/vendor/marked/marked.min.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .is_some()
    );
}

/// D3: `/static/previews/*` serves the embedded table with an image
/// `Content-Type` and cache, and answers 404 for anything uncommitted —
/// traversal-shaped paths included, because the lookup compares against
/// fixed keys and never touches the filesystem.
#[tokio::test]
async fn serves_committed_previews_and_404s_the_rest() {
    let ok = preview_asset(AxumPath("stat.svg".into())).await;
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(
        ok.headers()
            .get(header::CONTENT_TYPE)
            .expect("content-type")
            .to_str()
            .expect("ascii"),
        "image/svg+xml"
    );
    assert_eq!(
        ok.headers().get(header::CACHE_CONTROL).expect("cache"),
        "public, max-age=86400"
    );

    let missing = preview_asset(AxumPath("nope.svg".into())).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let traversal = preview_asset(AxumPath("../../Cargo.toml".into())).await;
    assert_eq!(traversal.status(), StatusCode::NOT_FOUND);

    // The route is registered where the auth guard lets assets through:
    // a sessionless request must not be bounced to /login.
    let app = Router::new()
        .route("/static/previews/{*path}", get(preview_asset))
        .layer(from_fn(security_headers));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/static/previews/generic.svg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .is_some()
    );
}

/// Regression test for the dev `/pkg` cache policy.
use axum::http::Request as HttpRequest;

/// The dev server rewrites the hydrate bundle in place, so a browser that
/// reuses a previous `ferris-web.js` can pair it with the freshly built wasm
/// and abort with `LinkError: import object field ... is not a Function`.
/// `pkg_no_store` is what prevents that: every bundle response must forbid
/// caching. The directory is deliberately absent — the header has to be
/// there even on the 404 from a bundle that has not been built yet.
#[tokio::test]
async fn pkg_responses_forbid_caching() {
    let app: Router = Router::new().nest_service(
        "/pkg",
        Router::new()
            .fallback_service(tower_http::services::ServeDir::new(
                std::env::temp_dir().join("ferris-pkg-does-not-exist"),
            ))
            .layer(from_fn(pkg_no_store)),
    );

    let response = app
        .oneshot(
            HttpRequest::builder()
                .uri("/pkg/ferris-web.js")
                .body(Body::empty())
                .expect("the request builds"),
        )
        .await
        .expect("the router answers");

    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store"),
        "the dev hydrate bundle must never be cached"
    );
}
