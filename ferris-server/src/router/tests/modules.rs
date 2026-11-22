//! The composition root mounts the real native modules.
//!
//! The unit tests in `ferris-print`/`ferris-report` prove the handlers; this
//! one proves the server's wiring: the registry, the `/api` mount, the shared
//! auth layer and the `OpenAPI` fragments all agree.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::FromRef;
use axum::http::{Method, Request, StatusCode, header};
use axum::middleware::from_fn_with_state;
use ferris_api::ApiState;
use ferris_auth::middleware::require_auth;
use ferris_auth::{AuthState, JwtConfig, JwtManager, SessionConfig, SessionManager};
use ferris_core::FeatureFlags;
use tower::ServiceExt;

use super::super::modules::native_modules;

#[derive(Clone)]
struct TestState {
    api: Arc<ApiState>,
    auth: AuthState,
}

impl FromRef<TestState> for Arc<ApiState> {
    fn from_ref(state: &TestState) -> Self {
        state.api.clone()
    }
}

impl FromRef<TestState> for AuthState {
    fn from_ref(state: &TestState) -> Self {
        state.auth.clone()
    }
}

fn bearer(state: &TestState) -> String {
    let roles = vec!["System Manager".to_string()];
    format!(
        "Bearer {}",
        state
            .auth
            .jwt_manager
            .generate_tokens("user-1", "user@example.com", &roles, None, "")
            .expect("test token")
            .access_token
    )
}

async fn state(features: FeatureFlags) -> TestState {
    let pool = ferris_db::memory_db().await.expect("memory db");
    let auth = AuthState::new(
        JwtManager::new(JwtConfig::new("test-secret-that-is-at-least-32-chars-long").unwrap())
            .expect("jwt"),
        SessionManager::new(SessionConfig::default()),
        SessionConfig::default(),
    );
    TestState {
        api: Arc::new(ApiState::new(pool).with_features(features)),
        auth,
    }
}

fn app(state: &TestState, modules: &[Arc<dyn ferris_api::Module>]) -> Router {
    let router: Router<TestState> = Router::new().merge(
        ferris_api::api_router_with_modules(state.clone(), modules)
            .layer(from_fn_with_state(state.auth.clone(), require_auth)),
    );
    router.with_state(state.clone())
}

async fn request(
    app: Router,
    method: Method,
    uri: &str,
    state: &TestState,
) -> (StatusCode, String) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, bearer(state))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
}

#[tokio::test]
async fn the_registered_modules_are_mounted_and_documented() {
    let features = FeatureFlags::default();
    let modules = native_modules(&features);
    crate::catalog::register_doctype_catalog(&modules);
    let state = state(features).await;

    // The module registry contributes paths only when it is given to the
    // router; the OpenAPI fragments come from that same registry.
    let api = Arc::new(
        (*state.api)
            .clone()
            .with_openapi_fragments(modules.openapi_fragments()),
    );
    let state = TestState { api, ..state };

    let (status, body) = request(
        app(&state, modules.modules()),
        Method::GET,
        "/api/print/formats/Print%20Format",
        &state,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = request(
        app(&state, modules.modules()),
        Method::GET,
        "/api/report",
        &state,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = request(
        app(&state, modules.modules()),
        Method::GET,
        "/api/openapi.json",
        &state,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("/api/print/"), "{body}");
    assert!(body.contains("/api/report"), "{body}");

    // The host's auth layer wraps module routes too.
    let response = app(&state, modules.modules())
        .oneshot(
            Request::builder()
                .uri("/api/print/formats/Print%20Format")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
