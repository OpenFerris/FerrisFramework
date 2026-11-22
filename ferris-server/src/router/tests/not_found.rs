//! SRV13 regression: the HTML 404 is English and colour-token free.
use axum::{
    body::Body,
    extract::Request,
    http::{StatusCode, header},
};

use super::super::fallback::not_found_fallback;

#[tokio::test]
async fn the_html_404_is_english_and_has_no_hardcoded_colours() {
    let request = Request::builder()
        .uri("/missing")
        .header(header::ACCEPT, "text/html")
        .body(Body::empty())
        .expect("the request builds");
    let response = not_found_fallback(request).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body is readable");
    let body = String::from_utf8(bytes.to_vec()).expect("the body is UTF-8");
    assert!(body.contains("does not exist on this server"));
    for forbidden in ["#", "rgb(", "hsl(", "oklch("] {
        assert!(
            !body.contains(forbidden),
            "the 404 page must not carry a literal colour ({forbidden}): {body}"
        );
    }
    assert!(
        body.contains("CanvasText"),
        "it uses system colours instead"
    );
}
