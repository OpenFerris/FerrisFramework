use axum::body::Body;

use super::super::pages::axum_page_path;
use super::super::plugin_http::to_wit_request;

/// P1 (WIT 2.0.0): everything the wire carries — method, query string
/// (no `?`), headers and a binary body — reaches the guest-shaped record
/// without the body ever passing through a lossy UTF-8 conversion.
#[tokio::test]
async fn request_carries_method_query_headers_and_binary_body() {
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/submit?doc=Task%201&flag=on")
        .header("x-probe-key", "yes")
        .header("content-type", "application/octet-stream")
        .body(Body::from(vec![0u8, 159, 146, 150, b'O', b'K']))
        .expect("request builds");

    let got = to_wit_request("/submit".to_string(), req, None)
        .await
        .expect("request converts");

    assert_eq!(got.method, "POST");
    assert_eq!(got.path, "/submit");
    assert_eq!(got.query, "doc=Task%201&flag=on");
    assert!(
        got.headers
            .contains(&("x-probe-key".to_string(), "yes".to_string()))
    );
    assert!(got.headers.contains(&(
        "content-type".to_string(),
        "application/octet-stream".to_string()
    )));
    assert_eq!(got.body, vec![0u8, 159, 146, 150, b'O', b'K']);
    assert!(got.client_identity.is_none());
}

/// No query string → empty `query` field (the 1.x `route` never had one).
#[tokio::test]
async fn absent_query_is_empty_not_none() {
    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/ping")
        .body(Body::empty())
        .expect("request builds");
    let got = to_wit_request("/ping".to_string(), req, None)
        .await
        .expect("request converts");
    assert_eq!(got.query, "");
    assert!(got.body.is_empty());
    assert!(got.headers.is_empty());
}

/// Declared plugin paths arrive in leptos/`:param` form; axum only
/// understands `{param}` (and `{*rest}` for splats). Getting this wrong
/// makes `leptos_routes` register a pattern axum rejects at boot.
#[test]
fn declared_plugin_paths_become_axum_patterns() {
    assert_eq!(axum_page_path("/status"), "/status");
    assert_eq!(axum_page_path("/blog/:slug"), "/blog/{slug}");
    assert_eq!(axum_page_path("/docs/*rest"), "/docs/{*rest}");
    assert_eq!(axum_page_path("/"), "/");
    // Trailing and doubled slashes collapse; `{}` segments pass through.
    assert_eq!(axum_page_path("status/"), "/status");
    assert_eq!(axum_page_path("/blog/{slug}"), "/blog/{slug}");
}
