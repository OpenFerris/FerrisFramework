//! App-owned namespaces: `/api/plugins/<app>/*` and `/desk/<app>/*`.
//!
//! Every app owns its own URL space. An app routes freely *inside* it — the host only resolves the app
//! name, enforces the Desk session for pages, resolves the declared route policy
//! (`none`/`session`/`capability`) for the API and forwards the relative path to
//! `handle-request` with a stable route id (`api` / `page`). Nothing under
//! `/api/plugins` or `/desk/<app>` can shadow a framework path: the first is a
//! framework-owned prefix added here, the second is registered **per app** as a
//! static segment.
//!
//! Hot reload: the page routes are built at boot, one pattern per loaded app.
//! An app that appears after a reload is reachable through `/api/plugins`
//! immediately (one catch-all) but its `/desk/<app>` pages need a restart, like
//! every declared route (`plugin_routes::mod`).

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use ferris_auth::User;
use ferris_wasm::descriptor::{NetworkSpec, RouteSpec};
use ferris_wasm::{HttpRequest, PluginStore, RouteAuth};

use super::plugin_http::{PluginUser, plugin_response, to_wit_request_with_limit};
use super::plugins::run_plugin;
use super::state::AppState;
use crate::client_identity::PluginClientIdentity;

/// Route id the guest receives for `/api/plugins/<app>/…`.
pub(super) const API_ROUTE_ID: &str = "api";

/// Route id the guest receives for `/desk/<app>/…`.
pub(super) const PAGE_ROUTE_ID: &str = "page";

/// The framework-owned API namespace of one app.
///
/// Mounted outside the `/api/*` router because `/api/plugins` is not a child the
/// framework's resource permissions own, and the app's own JSON API must not
/// inherit them. It carries **no** `require_auth_or_device`/`require_csrf`
/// layer: the mount only resolves the optional caller (`bind_plugin_user`) and
/// the mTLS facts (`bind_client_identity`), and [`dispatch`] enforces the policy
/// the app declares for the matched route — `session` (with CSRF on cookie
/// mutations), `capability`, or an explicitly public `none`, defaulting to
/// `session` for anything undeclared. A layer cannot decide per path, and a
/// router-wide session gate would make the documented public webhook impossible.
pub(super) fn api_router() -> Router<AppState> {
    Router::new().route("/api/plugins/{app}/{*rest}", any(api_dispatch))
}

/// Build the per-app page routes. Each app gets a **static** first segment
/// (`/desk/<app>`), which is the only shape that can coexist with the Desk's
/// `/desk/{module}/{doctype}` parameter routes.
///
/// A name that is already a `DocType` or a module is refused: `/desk/<that>`
/// is the framework's, and a static route would shadow the list/form pages of
/// the doctype. The refusal is logged (fail-closed), never silently mounted.
#[cfg(feature = "web")]
pub(super) fn desk_router(plugins: &Arc<PluginStore>) -> Router<AppState> {
    let mut router = Router::new();
    for plugin in plugins.plugins() {
        let name = plugin.name().to_string();
        if let Err(reason) = page_namespace_available(&name) {
            tracing::warn!(app = %name, %reason, "app page namespace not mounted");
            continue;
        }
        router = router
            .route(&format!("/desk/{name}"), get(page_dispatch))
            .route(&format!("/desk/{name}/{{*rest}}"), get(page_dispatch));
    }
    router
}

/// Why `/desk/<name>` cannot be an app namespace, if it cannot.
#[cfg(feature = "web")]
fn page_namespace_available(name: &str) -> Result<(), String> {
    if ferris_schema::get_doctype(name).is_some() {
        return Err(format!("a DocType called `{name}` defines /desk/<name>"));
    }
    if module_exists(name) {
        return Err(format!("a module called `{name}` defines /desk/<name>/*"));
    }
    Ok(())
}

/// Whether `name` is the module segment of any registered doctype
/// (`/desk/<module>/<doctype>`).
#[cfg(feature = "web")]
fn module_exists(name: &str) -> bool {
    ferris_schema::list_doctypes_metas()
        .iter()
        .any(|meta| meta.module.eq_ignore_ascii_case(name))
}

async fn api_dispatch(
    State(app): State<AppState>,
    Path((name, rest)): Path<(String, String)>,
    req: Request<Body>,
) -> Response {
    dispatch(&app, &name, rest, req, true).await
}

#[cfg(feature = "web")]
async fn page_dispatch(
    State(app): State<AppState>,
    Path((name, rest)): Path<(String, String)>,
    req: Request<Body>,
) -> Response {
    // Pages are a document surface: a session is required (`web_auth_guard`
    // is applied where the router is mounted) and the guest gets the same
    // caller identity the Desk has.
    dispatch(&app, &name, rest, req, false).await
}

/// The auth policy for `path`, from the app's declared routes.
///
/// A declared route applies when its method matches (`*` included) and its
/// `path` is a segment-wise prefix; the longest match wins (the same rule the
/// router uses). No match is `session`: the namespace never becomes public by
/// accident.
fn resolve_policy(network: Option<&NetworkSpec>, method: &str, path: &str) -> (String, RouteAuth) {
    let Some(network) = network else {
        return (API_ROUTE_ID.to_string(), RouteAuth::Session);
    };
    let mut best: Option<&RouteSpec> = None;
    for route in &network.routes {
        if !method_matches(&route.method, method) || !path_prefix(&route.path, path) {
            continue;
        }
        if best.is_none_or(|current| route.path.len() > current.path.len()) {
            best = Some(route);
        }
    }
    match best {
        Some(route) => (
            route.id.clone(),
            route
                .auth
                .clone()
                .unwrap_or_else(|| network.auth_mode.clone()),
        ),
        None => (API_ROUTE_ID.to_string(), RouteAuth::Session),
    }
}

fn method_matches(declared: &str, method: &str) -> bool {
    declared == "*" || declared.eq_ignore_ascii_case(method)
}

fn path_prefix(declared: &str, path: &str) -> bool {
    if declared == "/" {
        return true;
    }
    let declared = declared.trim_end_matches('/');
    path == declared || path.starts_with(&format!("{declared}/"))
}

/// CSRF for cookie-authenticated mutations, mirroring the API middleware:
/// safe methods, no session cookie and bearer callers are exempt.
async fn csrf_ok(app: &AppState, headers: &axum::http::HeaderMap) -> bool {
    let cookie_name = &app.auth.config.cookie_name;
    let Some(sid) = headers
        .get(axum::http::header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| {
            raw.split(';')
                .filter_map(|pair| pair.trim().split_once('='))
                .find(|(name, _)| *name == cookie_name)
                .map(|(_, value)| value.to_string())
        })
    else {
        return true;
    };
    let Some(token) = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    app.auth
        .session_manager
        .validate_csrf_token(&sid, token)
        .await
        .unwrap_or(false)
}

/// A `capability(name)` route: the caller sends the provisioned token in
/// `x-ferris-capability`; an unprovisioned capability is refused (403), not
/// treated as a bad token.
async fn capability_ok(
    plugins: &PluginStore,
    name: &str,
    capability: &str,
    headers: &axum::http::HeaderMap,
) -> bool {
    use ferris_core::Value;
    use ferris_db::Stmt;
    let key = format!("route.{capability}.token");
    let row = plugins
        .pool()
        .fetch_optional(&Stmt::new(
            "SELECT value FROM tabAppConfig WHERE plugin = ? AND key = ?",
            vec![Value::from(name), Value::from(key.as_str())],
        ))
        .await
        .ok()
        .flatten();
    let Some(expected) = row.and_then(|row| row.opt_str("value")) else {
        return false;
    };
    let supplied = headers
        .get("x-ferris-capability")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    supplied.len() == expected.len()
        && supplied
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// Forward one request to the app that owns the first path segment.
async fn dispatch(
    app: &AppState,
    name: &str,
    rest: String,
    req: Request<Body>,
    api: bool,
) -> Response {
    let Some(plugins) = app.plugins.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(plugin) = plugins.get(name) else {
        tracing::warn!(app = %name, "app namespace request for an app that is not loaded");
        return StatusCode::NOT_FOUND.into_response();
    };

    let user: Option<User> = req
        .extensions()
        .get::<PluginUser>()
        .and_then(|pu| pu.0.clone());
    let client_identity = req
        .extensions()
        .get::<PluginClientIdentity>()
        .and_then(|pi| pi.0.clone());

    let path = if rest.is_empty() {
        "/".to_string()
    } else {
        format!("/{rest}")
    };

    // The app declares the policy next to each route; `/api/plugins/<app>/…`
    // uses the same table. Unmatched paths are `session` (fail-closed), so an
    // app that wants a public endpoint says so.
    let (route_id, auth) = if api {
        resolve_policy(plugin.network().as_ref(), req.method().as_str(), &path)
    } else {
        // Pages are a Desk surface: always a session, always the Desk
        // `web_auth_guard` at mount; the app does not get to make one public.
        (PAGE_ROUTE_ID.to_string(), RouteAuth::Session)
    };
    let mutating = !matches!(
        *req.method(),
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    );
    match &auth {
        RouteAuth::None => {}
        RouteAuth::Session => {
            if user.is_none() {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            if mutating && !csrf_ok(app, req.headers()).await {
                return StatusCode::FORBIDDEN.into_response();
            }
        }
        RouteAuth::Capability(capability) => {
            if !capability_ok(plugins.as_ref(), name, capability, req.headers()).await {
                return StatusCode::UNAUTHORIZED.into_response();
            }
        }
    }
    // No plugin-specific byte cap: the request is bounded by the host's global
    // body limit (`[server] body_limit_mb`), the same ceiling every other route
    // of this process has. A cap here would silently truncate an app's API.
    let request: HttpRequest =
        match to_wit_request_with_limit(path, req, client_identity, usize::MAX).await {
            Ok(request) => request,
            Err(status) => return status.into_response(),
        };

    let plugin = name.to_string();
    let route = route_id;
    let outcome =
        run_plugin(move || plugins.handle_request_for_route(&plugin, &route, &request, user)).await;

    match outcome {
        Ok(reply) => plugin_response(reply),
        Err(error) => {
            let status = error
                .downcast_ref::<ferris_wasm::WasmError>()
                .and_then(ferris_wasm::WasmError::status)
                .and_then(|code| StatusCode::from_u16(code).ok())
                .unwrap_or(StatusCode::BAD_REQUEST);
            tracing::warn!(app = %name, %error, "app namespace request failed");
            status.into_response()
        }
    }
}
