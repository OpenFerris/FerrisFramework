//! Static assets served by the dev server: the desk stylesheet, the committed
//! vendor/preview tables and the `/pkg` hydrate bundle.

use axum::{
    extract::{Path as AxumPath, Request},
    http::HeaderValue,
    http::StatusCode,
    http::header::{self, CONTENT_TYPE},
    middleware,
    response::{IntoResponse, Response},
};

/// `Cache-Control: no-store` for the `/pkg` hydrate bundle.
///
/// This binary only ever runs as the `ferris dev --hot-reload front` server, so
/// the bundle is rebuilt in place and the browser must not reuse a previous
/// `ferris-web.js` against the new wasm. wasm-bindgen resolves imports by name
/// at instantiation time, so a glue predating a new `web-sys` call leaves the
/// wasm asking for an import it does not provide: the page dies with
/// `LinkError: import object field '__wbg_...' is not a Function`.
pub(super) async fn pkg_no_store(request: Request, next: middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Serves the desk stylesheet compiled into the `ferris-web` binary.
pub(super) async fn desk_css() -> Response {
    (
        [
            (CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        ferris_web::DESK_CSS,
    )
        .into_response()
}

/// Serves one committed vendor library file (`/static/vendor/{*path}`), from
/// the same embedded table the production router uses. Unknown path → 404.
pub(super) async fn vendor_asset(AxumPath(path): AxumPath<String>) -> Response {
    match ferris_web::vendor_assets::lookup(&path) {
        Some((content_type, bytes)) => (
            [
                (CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Serves one widget-catalog preview (`/static/previews/{*path}`), from the
/// same embedded table the production router uses. Unknown path → 404 (the
/// card's `on:error` fallback then paints `generic.svg`).
pub(super) async fn preview_asset(AxumPath(path): AxumPath<String>) -> Response {
    match ferris_web::preview_assets::lookup(&path) {
        Some((content_type, bytes)) => (
            [
                (CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
