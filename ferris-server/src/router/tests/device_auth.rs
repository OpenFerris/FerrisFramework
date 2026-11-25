use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{Request as HttpRequest, StatusCode},
    middleware::{from_fn, from_fn_with_state},
};
use ferris_auth::middleware::require_csrf;
use ferris_auth::{AuthState, SessionConfig};
use ferris_core::ClientIdentity;

use super::super::auth_layers::require_auth_or_device;
use crate::client_identity::bind_client_identity;
use crate::tls::TlsClientInfo;
use std::net::SocketAddr;
use tower::ServiceExt;

fn auth_state() -> AuthState {
    AuthState::new(SessionConfig::default())
}

fn identity() -> ClientIdentity {
    ClientIdentity {
        subject: "O=Solmec,CN=SOLMEC-001".into(),
        common_name: Some("SOLMEC-001".into()),
        fingerprint: "ab".repeat(32),
        issuer: "CN=Ferris Test Root CA".into(),
    }
}

/// The production layer order for the API router: `bind_client_identity`
/// outermost, then the device-aware auth gate, then the CSRF gate inside
/// (exactly as `api_router_with_app_state` mounts it).
fn app() -> Router {
    let auth = auth_state();
    Router::new()
        .route(
            "/api/method/solmecbox.ingest",
            axum::routing::post(|| async { "ok" }),
        )
        .route("/api/resource/Task", axum::routing::post(|| async { "ok" }))
        .layer(from_fn_with_state(auth.clone(), require_csrf))
        .layer(from_fn_with_state(auth, require_auth_or_device))
        .layer(from_fn(bind_client_identity))
}

fn request(path: &str, with_identity: bool) -> HttpRequest<Body> {
    let mut request = HttpRequest::builder()
        .method("POST")
        .uri(path)
        .body(Body::empty())
        .expect("request builds");
    if with_identity {
        request.extensions_mut().insert(ConnectInfo(TlsClientInfo {
            peer: "127.0.0.1:12345".parse::<SocketAddr>().unwrap(),
            identity: Some(identity()),
        }));
    }
    request
}

/// H1 / Fase 2: a certificate-only caller reaches `/api/method/*` — the
/// positive case that used to answer 401 because the method path sat behind
/// the session-only `require_auth` gate.
#[tokio::test]
async fn a_verified_certificate_reaches_the_method_path() {
    let response = app()
        .oneshot(request("/api/method/solmecbox.ingest", true))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// No session and no certificate is still refused.
#[tokio::test]
async fn an_unauthenticated_method_call_is_refused() {
    let response = app()
        .oneshot(request("/api/method/solmecbox.ingest", false))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// The device credential is scoped to `/api/method/*`: on a resource route
/// it is not a substitute for a session, so the request is refused.
#[tokio::test]
async fn a_certificate_does_not_open_resource_routes() {
    let response = app()
        .oneshot(request("/api/resource/Task", true))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
