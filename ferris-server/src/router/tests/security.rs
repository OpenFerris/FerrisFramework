use axum::{
    Router,
    body::{Body, to_bytes},
    extract::Request,
    http::header,
    middleware::from_fn,
    response::Html,
    routing::get,
};

use super::super::security::{CspNonce, security_headers};

/// M8: the fusion router used to send no security headers at all, so any
/// XSS that reached the Desk ran with full DOM/session access and nothing
/// stopped cross-origin embedding. Every response must now carry the CSP
/// and the framing/sniffing/HSTS protections.
#[tokio::test]
async fn every_response_carries_the_full_security_header_set() {
    use tower::ServiceExt;

    let app = Router::new()
        .route("/", get(|| async { Html("<h1>ok</h1>") }))
        .layer(from_fn(security_headers));

    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    let headers = response.headers();
    let csp = headers
        .get(header::CONTENT_SECURITY_POLICY)
        .expect("CSP present")
        .to_str()
        .unwrap();
    // The policy is strict default-deny plus a per-response nonce for the
    // inline scripts Leptos emits; nothing but images/fonts is embeddable.
    assert!(csp.contains("default-src 'self'"));
    assert!(csp.contains("script-src 'self' 'nonce-"));
    assert!(csp.contains("'wasm-unsafe-eval'"));
    assert!(csp.contains("frame-ancestors 'self'"));
    assert!(csp.contains("object-src 'none'"));
    // T2-F4: images stay same-origin/data/blob except the single named
    // host the Geolocation map loads tiles from — assert the exact
    // directive so a stray `https:` wildcard cannot slip in unnoticed.
    assert!(
        csp.contains("img-src 'self' data: blob: https://tile.openstreetmap.org;"),
        "img-src must allow exactly the OSM tile host, got: {csp}"
    );

    assert_eq!(headers.get(header::X_FRAME_OPTIONS).unwrap(), "SAMEORIGIN");
    assert_eq!(
        headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
        "nosniff"
    );
    assert_eq!(
        headers.get(header::REFERRER_POLICY).unwrap(),
        "strict-origin-when-cross-origin"
    );
    assert_eq!(
        headers.get(header::STRICT_TRANSPORT_SECURITY).unwrap(),
        "max-age=31536000; includeSubDomains"
    );
}

/// The CSP nonce must reach the render: the middleware puts it on the
/// request, the handler (standing in for `provide_desk_context`) reads it
/// back, and the header advertises that exact value — otherwise Leptos'
/// inline bootstrap is blocked and the Desk never hydrates. It must also be
/// fresh per response so a leaked script tag can't be replayed.
#[tokio::test]
async fn csp_nonce_reaches_the_handler_and_is_fresh_per_response() {
    use axum::extract::Extension;
    use tower::ServiceExt;

    async fn probe(Extension(nonce): Extension<CspNonce>) -> String {
        nonce.0.to_string()
    }

    let app = Router::new()
        .route("/probe", get(probe))
        .layer(from_fn(security_headers));

    let run = || {
        app.clone().oneshot(
            Request::builder()
                .uri("/probe")
                .body(Body::empty())
                .unwrap(),
        )
    };

    let (parts, body) = run().await.unwrap().into_parts();
    let nonce = String::from_utf8(to_bytes(body, usize::MAX).await.unwrap().to_vec()).unwrap();
    let csp = parts
        .headers
        .get(header::CONTENT_SECURITY_POLICY)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        csp.contains(&format!("'nonce-{nonce}'")),
        "header must advertise the nonce the handler saw: {csp:?} vs {nonce:?}"
    );

    let second = to_bytes(run().await.unwrap().into_body(), usize::MAX)
        .await
        .unwrap();
    assert_ne!(
        nonce.as_bytes(),
        second.as_ref(),
        "the nonce must be minted fresh for every response"
    );
}
