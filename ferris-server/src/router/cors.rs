//! CORS policy for the fused router.

use axum::http::{HeaderValue, Method, header};
use tower_http::cors::CorsLayer;
use tracing::warn;

use crate::server::ServerSettings;

pub(super) fn build_cors_layer(settings: &ServerSettings) -> CorsLayer {
    // Allow-list only: a `*` entry would turn the router into a
    // wide-open endpoint, so it is refused instead of silently expanded.
    let mut origins = Vec::with_capacity(settings.cors_origins.len());
    for origin in &settings.cors_origins {
        if origin.trim() == "*" {
            warn!("CORS origin '*' is refused; list explicit origins instead (cors_origins)");
            continue;
        }
        match origin.parse::<HeaderValue>() {
            Ok(v) => origins.push(v),
            Err(_) => warn!("ignoring invalid CORS origin in settings: {origin:?}"),
        }
    }

    // Explicit method/header set, not `Any`: keeps preflight responses tight
    // and lists exactly what the desk needs. Credentials are allowed because
    // the session cookie carries authentication, which forces
    // explicit origins above. `x-csrf-token` is both allowed and exposed: the
    // desk reads it off `login`/`session` responses and echoes it back on
    // mutating calls.
    CorsLayer::new()
        .allow_origin(origins)
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::ACCEPT,
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            ferris_auth::CSRF_HEADER,
        ])
        .expose_headers([ferris_auth::CSRF_HEADER])
}
