//! Plugin HTTP surface: describe, UI assets/widgets and request forwarding.

use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    extract::{Extension, FromRequestParts, Path as AxumPath, Query as AxumQuery, Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::Response,
};
use ferris_auth::AuthState;
use ferris_auth::User;
use ferris_auth::middleware::OptionalUser;
use ferris_core::ClientIdentity;
use ferris_wasm::{
    HttpRequest, PluginStore, Response as PluginResponse, WasmError, sanitize_response_headers,
};
use serde::Deserialize;
use tracing::warn;

use super::plugins::{PLUGIN_BODY_LIMIT_BYTES, run_plugin};
use crate::client_identity::PluginClientIdentity;

/// Ambient identity bound to plugin HTTP requests. Resolved from the desk
/// session (bearer token or cookie) before the plugin runs; `None` means the
/// caller was anonymous, which the plugin's `session` import reports as a
/// guest rather than a fabricated identity.
#[derive(Clone)]
pub(super) struct PluginUser(pub(super) Option<User>);

/// Resolve the optional session user for plugin routes and stash it in request
/// extensions, without rejecting anonymous callers. Plugins keep working for
/// public endpoints, but `session::current` inside a logged-in request now
/// returns the real user (id/email/roles) instead of always a guest — the id
/// is what the plugin uses to target in-app notifications at the caller.
pub async fn bind_plugin_user(
    State(auth): State<AuthState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, Response> {
    let (mut parts, body) = request.into_parts();
    let user = OptionalUser::from_request_parts(&mut parts, &auth)
        .await
        .map(|extractor| extractor.0)
        // `OptionalUser::Rejection` is `Infallible`; spelling the arm out keeps
        // the request path free of `expect` (SRV12).
        .unwrap_or_else(|never| match never {});
    parts.extensions.insert(PluginUser(user));
    let request = Request::from_parts(parts, body);
    Ok(next.run(request).await)
}

pub(super) async fn plugin_describe(
    State(plugins): State<Arc<PluginStore>>,
    AxumPath(name): AxumPath<String>,
) -> Result<axum::response::Response, StatusCode> {
    // The wasm call is blocking and its host imports need a tokio context, so
    // park this worker out of the scheduler while it runs. `describe_json_for`
    // projects the typed WIT `descriptor` record onto the stable JSON surface
    // `/plugins/{name}` has served since 1.x, resolving the app by name (SRV17).
    let desc = run_plugin(move || plugins.describe_json_for(&name)).await;
    match desc {
        Ok(desc) => axum::response::Response::builder()
            .header("content-type", "application/json")
            .body(Body::from(desc))
            .map_err(|error| {
                tracing::error!(%error, "building the plugin describe response failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

/// `GET /plugins/{name}/asset/{*path}` — relay one of the loaded plugin's
/// UI assets (script, stylesheet, image…) from its `ui-asset` export. The
/// content type is guessed from the path extension; unknown extensions fall
/// back to `application/octet-stream`.
pub(super) async fn plugin_ui_asset(
    State(plugins): State<Arc<PluginStore>>,
    AxumPath((name, path)): AxumPath<(String, String)>,
) -> Result<axum::response::Response, StatusCode> {
    let content_type = asset_content_type(&path);
    // The `Widget` DocType owns widget source. When the row declares this
    // asset path, the stored script wins over the app's embedded copy, so a
    // widget edited in the Desk is served at its original URL and relative
    // `import`s keep resolving. Only `.js` is looked up: the app's shared
    // runtime and stylesheets still come from `ui-asset`.
    let is_script = std::path::Path::new(&path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("js"));
    if is_script
        && let Some(script) =
            ferris_api::widgets::script_by_path(plugins.pool(), &name, &path).await
    {
        return axum::response::Response::builder()
            .header("content-type", content_type)
            .header("cache-control", "no-store, max-age=0")
            .body(Body::from(script))
            .map_err(|error| {
                tracing::error!(%error, "building the plugin asset response failed");
                StatusCode::INTERNAL_SERVER_ERROR
            });
    }
    let bytes = run_plugin(move || plugins.ui_asset(&name, &path)).await;
    match bytes {
        Ok(bytes) => axum::response::Response::builder()
            .header("content-type", content_type)
            .body(Body::from(bytes))
            .map_err(|error| {
                tracing::error!(%error, "building the plugin asset response failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

/// Query string of [`plugin_ui_widget`]: `params` carries the instance's
/// (name, value) pairs as a JSON array of pairs, URL-encoded — the same
/// values the mounted element receives in `data-params`.
#[derive(Debug, Default, Deserialize)]
pub(super) struct WidgetSsrQuery {
    #[serde(default)]
    params: Option<String>,
}

/// `GET /plugins/{name}/widget/{widget}?params=…` — SSR markup for one widget
/// *definition* rendered with an instance's parameters, via the plugin's
/// `ui-widget` export. The Desk prints it before the widget's custom element
/// hydrates.
pub(super) async fn plugin_ui_widget(
    State(plugins): State<Arc<PluginStore>>,
    AxumPath((name, widget)): AxumPath<(String, String)>,
    Extension(PluginUser(user)): Extension<PluginUser>,
    AxumQuery(query): AxumQuery<WidgetSsrQuery>,
) -> Result<axum::response::Response, StatusCode> {
    let params: Vec<(String, String)> = query
        .params
        .as_deref()
        .filter(|raw| !raw.is_empty())
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .unwrap_or_default();
    // Bind the ambient session so SSR that reads documents is authorized as
    // the real caller — the same identity the live widget will read as.
    let html = run_plugin(move || plugins.ui_widget_as(&name, &widget, &params, user)).await;
    match html {
        Ok(html) => axum::response::Response::builder()
            .header("content-type", "text/html; charset=utf-8")
            .body(Body::from(html))
            .map_err(|error| {
                tracing::error!(%error, "building the plugin widget response failed");
                StatusCode::INTERNAL_SERVER_ERROR
            }),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

/// Map an asset path extension to a content type for `plugin_ui_asset`.
fn asset_content_type(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "html" | "htm" => "text/html; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Forward an arbitrary `<app>/<rest>` request to the guest (dev only).
///
/// The production ingress is the declared-route router (per-route auth) and
/// `/api/plugins` (declared policy); `build_router` mounts this catch-all only
/// when `[server] dev` is on because it forwards every path under the fixed
/// `plugins` route id with an optional caller.
pub(super) async fn plugin_handle_request(
    State(plugins): State<Arc<PluginStore>>,
    AxumPath((name, rest)): AxumPath<(String, String)>,
    req: Request<Body>,
) -> Result<axum::response::Response, StatusCode> {
    // Ambient session user resolved by `bind_plugin_user` (`None` when the
    // caller is anonymous). Forwarded so `session::current` inside the guest
    // reports the real id — the value the plugin uses to target notifications.
    let user = req
        .extensions()
        .get::<PluginUser>()
        .and_then(|pu| pu.0.clone());
    // Verified mTLS certificate facts for this connection, stashed by
    // `bind_client_identity` (`None` for plain HTTP or an anonymous client).
    let client_identity = req
        .extensions()
        .get::<PluginClientIdentity>()
        .and_then(|pi| pi.0.clone());

    let path = if rest.is_empty() {
        "/".to_string()
    } else {
        format!("/{rest}")
    };
    let request = to_wit_request(path, req, client_identity).await?;

    let outcome = run_plugin(move || plugins.handle_request_as(&name, &request, user)).await;

    match outcome {
        Ok(reply) => Ok(plugin_response(reply)),
        Err(e) => {
            // A typed `WasmError` carries the semantic status the guest chose
            // (`PluginError::status()`: 404/403/422/409/429, traps → 500).
            // Host-side guards ("no plugin loaded", name mismatch) still mean
            // the plugin cannot serve this route → 404. Anything else keeps the
            // historical 400.
            let status = e
                .downcast_ref::<WasmError>()
                .and_then(WasmError::status)
                .or_else(|| {
                    let msg = e.to_string();
                    (msg.contains("not loaded") || msg.contains("mismatch"))
                        .then_some(StatusCode::NOT_FOUND.as_u16())
                })
                .and_then(|code| StatusCode::from_u16(code).ok())
                .unwrap_or(StatusCode::BAD_REQUEST);
            Err(status)
        }
    }
}

/// Build the host response from the plugin's typed `http.response`: status
/// and body verbatim, headers after [`sanitize_response_headers`] (hop-by-hop
/// and framework-owned pairs dropped), defaulting to the 1.0
/// `text/plain; charset=utf-8` when the plugin set no `content-type`.
pub(super) fn plugin_response(resp: PluginResponse) -> axum::response::Response {
    let headers = sanitize_response_headers(&resp.headers);
    let status = StatusCode::from_u16(resp.status).unwrap_or_else(|_| {
        warn!(
            status = resp.status,
            "plugin returned an invalid status; answering 500"
        );
        StatusCode::INTERNAL_SERVER_ERROR
    });
    let mut builder = axum::response::Response::builder().status(status);
    if !headers.iter().any(|(name, _)| name == "content-type") {
        builder = builder.header(header::CONTENT_TYPE, "text/plain; charset=utf-8");
    }
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    match builder.body(Body::from(resp.body)) {
        Ok(response) => response,
        Err(error) => {
            // `sanitize_response_headers` validates every pair, so this is
            // defence in depth: a rejected header must not panic the worker
            // (SRV12).
            warn!(%error, "plugin response headers were rejected; answering 500");
            let mut response =
                axum::response::Response::new(Body::from("invalid plugin response headers"));
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            response
        }
    }
}

/// Project an axum request into the typed `HttpRequest` the guest sees:
/// method and path verbatim, the query string **without** the leading `?`
/// (absent → empty), header values as lossy UTF-8 (a `string` field cannot
/// carry raw header bytes), the body as **raw bytes** — no UTF-8
/// round-trip, so uploads arrive intact — and the verified client-certificate
/// identity, when the caller presented one.
pub(super) async fn to_wit_request(
    path: String,
    req: Request<Body>,
    client_identity: Option<ClientIdentity>,
) -> Result<HttpRequest, StatusCode> {
    to_wit_request_with_limit(path, req, client_identity, PLUGIN_BODY_LIMIT_BYTES).await
}

/// [`to_wit_request`] with the route's own body cap.
///
/// A declared route may ask for a smaller limit than the host-wide plugin cap
/// (`types.route.body-limit`); the caller has already clamped it, so this is the
/// value `to_bytes` enforces. An oversized body is `413` before the guest sees
/// any of it.
pub(super) async fn to_wit_request_with_limit(
    path: String,
    req: Request<Body>,
    client_identity: Option<ClientIdentity>,
    limit: usize,
) -> Result<HttpRequest, StatusCode> {
    let method = req.method().as_str().to_owned();
    let query = req.uri().query().unwrap_or_default().to_owned();
    let headers = req
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_owned(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = to_bytes(req.into_body(), limit)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    Ok(HttpRequest {
        method,
        path,
        query,
        headers,
        body: body.to_vec(),
        client_identity: client_identity.map(ferris_wasm::wit_client_identity),
    })
}
