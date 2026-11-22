//! Embedded front-end assets: the desk stylesheet, vendor libraries and widget
//! previews.

use axum::{
    extract::{Path as AxumPath, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::state::AppState;

/// Forbids caching of the dev hydrate bundle served at `/pkg`.
///
/// The bundle is rebuilt in place, but keeps the same URLs, so an HTTP-cached
/// `ferris-web.js` from a previous build can be paired with the wasm from the
/// current one. wasm-bindgen imports are resolved by name at instantiation
/// time, and a new `web-sys` call (a `setInterval`, a pointer event, ...) adds
/// an import the old glue does not provide, which aborts the page with
/// `LinkError: import object field '__wbg_...' is not a Function`. `no-store`
/// makes the browser re-fetch both files on every load; the bundle is tiny next
/// to the wasm next to it, and this only runs in dev.
pub(super) async fn pkg_no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Serves the desk stylesheet. In dev mode (`desk_css_dev` set) it streams the
/// freshly-tailwinded file with `no-store` so theme edits show up on refresh
/// without a server restart; otherwise it returns the CSS compiled into the
/// `ferris-web` binary.
pub(super) async fn desk_css(State(state): State<AppState>) -> axum::response::Response {
    if let Some(path) = &state.leptos.config.desk_css_dev {
        if path.as_ref().is_file() {
            if let Ok(bytes) = tokio::fs::read(path.as_ref()).await {
                return (
                    [
                        ("content-type", "text/css; charset=utf-8"),
                        ("cache-control", "no-store"),
                    ],
                    bytes,
                )
                    .into_response();
            }
        }
    }
    (
        [("content-type", "text/css; charset=utf-8")],
        ferris_web::DESK_CSS,
    )
        .into_response()
}

/// Serves one committed vendor library file (`/static/vendor/{*path}`).
///
/// Every byte comes from `ferris_web::vendor_assets` — compiled into this
/// binary — so a deployment needs no `node_modules` and no directory layout
/// next to it, and an unknown path answers 404 without touching the disk.
/// The one-day cache is safe because the files only change when a pinned
/// version is bumped (and the binary rebuilt with them).
pub(super) async fn vendor_asset(AxumPath(path): AxumPath<String>) -> axum::response::Response {
    match ferris_web::vendor_assets::lookup(&path) {
        Some((content_type, bytes)) => (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Serves one committed widget-catalog preview (`/static/previews/{*path}`).
///
/// Every byte comes from `ferris_web::preview_assets` — compiled into this
/// binary — so the cards' images need no directory layout next to it, and an
/// unknown path answers 404 (the `<img>` `on:error` fallback then paints
/// `generic.svg` from the same table).
pub(super) async fn preview_asset(AxumPath(path): AxumPath<String>) -> axum::response::Response {
    match ferris_web::preview_assets::lookup(&path) {
        Some((content_type, bytes)) => (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
