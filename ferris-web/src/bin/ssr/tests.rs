use super::plugin::to_wit_request;
use axum::body::Body;

/// P1 (WIT 2.0.0), SSR twin: method, query (no `?`), headers and a
/// binary body reach the guest-shaped record intact.
#[tokio::test]
async fn request_carries_method_query_headers_and_binary_body() {
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/submit?doc=Task%201&flag=on")
        .header("x-probe-key", "yes")
        .body(Body::from(vec![0u8, 159, 146, 150, b'O', b'K']))
        .expect("request builds");
    let got = to_wit_request("/submit".to_string(), req)
        .await
        .expect("request converts");
    assert_eq!(got.method, "POST");
    assert_eq!(got.path, "/submit");
    assert_eq!(got.query, "doc=Task%201&flag=on");
    assert!(
        got.headers
            .contains(&("x-probe-key".to_string(), "yes".to_string()))
    );
    assert_eq!(got.body, vec![0u8, 159, 146, 150, b'O', b'K']);
}
