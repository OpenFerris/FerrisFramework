//! Mounting the validated table in axum, and dispatching one request to the
//! guest that declared it.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry as MapEntry;

use axum::{
    Router,
    body::Body,
    extract::{Extension, FromRef, Request, State},
    http::StatusCode,
    response::Response,
    routing::{MethodRouter, delete, get, head, options, patch, post, put},
};
use ferris_wasm::descriptor::NetworkSpec;
use ferris_wasm::{PluginStore, RouteAuth};
use tracing::{error, info, warn};

use super::stream::declared_stream_handler;
use super::table::{MountedRoute, RouteTable};
use crate::client_identity::PluginClientIdentity;
use crate::router::plugin_http::{PluginUser, plugin_response, to_wit_request_with_limit};
use crate::router::plugins::run_plugin;
use crate::router::state::AppState;

/// The header a `capability` route reads its token from.
pub(super) const CAPABILITY_HEADER: &str = "x-ferris-capability";

/// The app config table the host writes (`config-set`, `tls-material.secret`).
/// Duplicated from `ferris-wasm`'s private constant on purpose: the router reads
/// the operator-visible row, and a rename must break both sides loudly.
const CONFIG_TABLE: &str = "tabAppConfig";

/// The slice of the application state a declared route needs.
///
/// Declared routes are served by two routers — the main one (state `AppState`)
/// and the dedicated TLS listener (state `DeclaredRouteState`) — so the handler
/// extracts this substate from either. `AppState` implements
/// [`FromRef`] for it, which is what lets the same `MethodRouter` be mounted in
/// both places without duplicating the dispatch.
#[derive(Clone)]
pub struct DeclaredRouteState {
    /// The loaded apps; `None` when this process publishes none.
    pub plugins: Option<std::sync::Arc<PluginStore>>,
    /// Session/CSRF state the edge layers need.
    pub auth: ferris_auth::AuthState,
}

impl FromRef<AppState> for DeclaredRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            plugins: state.plugins.clone(),
            auth: state.auth.clone(),
        }
    }
}

impl FromRef<DeclaredRouteState> for ferris_auth::AuthState {
    fn from_ref(state: &DeclaredRouteState) -> Self {
        state.auth.clone()
    }
}

/// Which declared routes a table mounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteMode {
    /// Plain HTTP: everything except `tls-server` routes and stream listeners.
    Plain,
    /// The dedicated TLS listener: only the routes that carry `tls-server`.
    Tls,
}

/// The context one mounted route hands its handler.
#[derive(Clone)]
pub(super) struct RouteContext {
    pub(super) app: String,
    pub(super) route_id: String,
    pub(super) path: String,
    pub(super) auth: RouteAuth,
    pub(super) body_limit: usize,
    /// `Some` for a `%stream` route: the chunk cap and idle timeout its listener
    /// declared. `None` is a request/response route.
    pub(super) stream: Option<StreamLimits>,
}

/// The limits a declared `%stream` route carries into its upgrade handler.
#[derive(Clone, Copy)]
pub(super) struct StreamLimits {
    pub(super) max_frame: Option<usize>,
    pub(super) idle: Option<std::time::Duration>,
}

/// Build the router for the declared routes, ready to merge.
///
/// Every refusal is logged: a route the operator believes is served and is not
/// is exactly the kind of silent gap this codebase refuses to ship.
///
/// The caller applies the auth layers (`bind_plugin_user`,
/// `bind_client_identity`) to the returned router, so they reach the declared
/// routes and nothing else.
pub fn build<S>(apps: &[(String, NetworkSpec)], mode: RouteMode) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    DeclaredRouteState: FromRef<S>,
{
    let table = RouteTable::build_for(apps, mode);
    for (app, route, conflict) in &table.refused {
        error!(
            app = %app,
            route = %route,
            reason = %conflict,
            "declared route not mounted (fail-closed)"
        );
    }
    let mut router: Router<S> = Router::new();
    if table.routes.is_empty() {
        return router;
    }

    // `pattern -> MethodRouter`, so two apps declaring different methods of the
    // same path merge into one route instead of panicking at boot.
    let mut by_pattern: BTreeMap<String, MethodRouter<S>> = BTreeMap::new();
    for route in &table.routes {
        let handler = method_router(route, route.context());
        for pattern in route.patterns() {
            match by_pattern.entry(pattern) {
                MapEntry::Vacant(slot) => {
                    slot.insert(handler.clone());
                }
                MapEntry::Occupied(mut slot) => {
                    let merged = slot.get().clone().merge(handler.clone());
                    *slot.get_mut() = merged;
                }
            }
        }
    }
    for (pattern, methods) in by_pattern {
        router = router.route(&pattern, methods);
    }
    let streams = table.routes.iter().filter(|route| route.stream).count();
    info!(
        routes = table.routes.len() - streams,
        stream_routes = streams,
        refused = table.refused.len(),
        "declared app routes mounted at the root"
    );
    router
}

/// The network declarations of every loaded app, in name order.
///
/// A plugin that has not described itself yet contributes nothing (and says so):
/// mounting a surface from a component whose `describe()` failed would be
/// guessing.
pub fn collect(plugins: &PluginStore) -> Vec<(String, NetworkSpec)> {
    let mut declarations = Vec::new();
    for plugin in plugins.plugins() {
        let name = plugin.name().to_string();
        match crate::router::plugins::run_blocking(|| plugins.network_for(&name)) {
            Ok(network) => declarations.push((name, network)),
            Err(e) => warn!(app = %name, error = %e, "app has no network declaration"),
        }
    }
    declarations
}

impl MountedRoute {
    fn context(&self) -> RouteContext {
        RouteContext {
            app: self.app.clone(),
            route_id: self.id.clone(),
            path: self.path.clone(),
            auth: self.auth.clone(),
            body_limit: self.body_limit,
            stream: self.stream.then(|| StreamLimits {
                max_frame: self.max_frame(),
                idle: self.idle(),
            }),
        }
    }
}

/// The method router for one declared method (or every method for `*`), each arm
/// carrying the route's context as an extension layer.
///
/// A `%stream` route has exactly one arm — the upgrade — because a WebSocket only
/// starts with a `GET`; the table already normalized its method.
fn method_router<S>(route: &MountedRoute, context: RouteContext) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
    DeclaredRouteState: FromRef<S>,
{
    let layer = Extension(context);
    if route.stream {
        return get(declared_stream_handler).layer(layer);
    }
    match route.method.as_str() {
        "*" => get(declared_route_handler)
            .post(declared_route_handler)
            .put(declared_route_handler)
            .patch(declared_route_handler)
            .delete(declared_route_handler)
            .options(declared_route_handler)
            .layer(layer),
        "GET" => get(declared_route_handler).layer(layer),
        "POST" => post(declared_route_handler).layer(layer),
        "PUT" => put(declared_route_handler).layer(layer),
        "PATCH" => patch(declared_route_handler).layer(layer),
        "DELETE" => delete(declared_route_handler).layer(layer),
        "HEAD" => head(declared_route_handler).layer(layer),
        _ => options(declared_route_handler).layer(layer),
    }
}

/// Serve one declared route.
///
/// Order matters and mirrors the `/plugins/*` path: authenticate, cap the body,
/// project the request, then hand it to the guest inside the blocking-call
/// guard. A request that fails an edge check never reaches the plugin.
async fn declared_route_handler(
    State(state): State<DeclaredRouteState>,
    Extension(context): Extension<RouteContext>,
    Extension(PluginUser(session_user)): Extension<PluginUser>,
    Extension(PluginClientIdentity(client_identity)): Extension<PluginClientIdentity>,
    req: Request<Body>,
) -> Result<Response, StatusCode> {
    let Some(plugins) = state.plugins.clone() else {
        // Mounted without a store is a wiring bug, but a 404 is the honest
        // answer: nothing can serve this path right now.
        warn!(route = %context.route_id, "declared route has no plugin store");
        return Err(StatusCode::NOT_FOUND);
    };

    let supplied = req
        .headers()
        .get(CAPABILITY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let user = authorize(&plugins, &context, session_user, supplied.as_deref()).await?;

    let path = tail(&context.path, req.uri().path());
    let request = to_wit_request_with_limit(
        path,
        strip_capability_header(req),
        client_identity,
        context.body_limit,
    )
    .await?;

    let app = context.app.clone();
    let route_id = context.route_id.clone();
    let outcome =
        run_plugin(move || plugins.handle_request_for_route(&app, &route_id, &request, user)).await;

    match outcome {
        Ok(reply) => Ok(plugin_response(reply)),
        Err(e) => {
            let status = e
                .downcast_ref::<ferris_wasm::WasmError>()
                .and_then(ferris_wasm::WasmError::status)
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

/// Enforce the route's auth and return the user the guest runs as.
///
/// `Ok(None)` is an anonymous dispatch (`none` and `capability` routes);
/// `Ok(Some(user))` is a session-authenticated one. Shared with the `%stream`
/// upgrade handler so an upgrade and a request cannot disagree about what an
/// auth mode means.
pub(super) async fn authorize(
    plugins: &PluginStore,
    context: &RouteContext,
    session_user: Option<ferris_auth::User>,
    supplied: Option<&str>,
) -> Result<Option<ferris_auth::User>, StatusCode> {
    match &context.auth {
        RouteAuth::None => Ok(None),
        RouteAuth::Session => session_user.map(Some).ok_or(StatusCode::UNAUTHORIZED),
        RouteAuth::Capability(name) => {
            let key = format!("route.{name}.token");
            let provisioned = read_config(plugins, &context.app, &key).await?;
            let Some(expected) = provisioned else {
                // The app asks for a capability it never provisioned: 403 tells
                // the operator to set the key, 401 would look like a bad token.
                warn!(
                    app = %context.app,
                    route = %context.route_id,
                    key = %key,
                    "capability route has no provisioned token"
                );
                return Err(StatusCode::FORBIDDEN);
            };
            let supplied = supplied.unwrap_or_default();
            if constant_time_eq(supplied.as_bytes(), expected.as_bytes()) {
                Ok(None)
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        }
    }
}

/// Read one of the app's config values, if it exists.
async fn read_config(
    plugins: &PluginStore,
    app: &str,
    key: &str,
) -> Result<Option<String>, StatusCode> {
    use ferris_core::Value;
    use ferris_db::Stmt;
    let row = plugins
        .pool()
        .fetch_optional(&Stmt::new(
            format!("SELECT value FROM {CONFIG_TABLE} WHERE plugin = ? AND key = ?"),
            vec![Value::from(app), Value::from(key)],
        ))
        .await
        .map_err(|e| {
            error!(app = %app, key = %key, error = %e, "reading the app config failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(row.and_then(|row| row.opt_str("value")))
}

/// Compare two secrets without an early exit.
///
/// The lengths are compared first (a length is not a secret); the byte loop has
/// no branch on the content, so the time does not tell an attacker how far a
/// guess matched.
pub(super) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in a.iter().zip(b) {
        diff |= left ^ right;
    }
    diff == 0
}

/// The guest-visible path: what follows the matched prefix, always starting
/// with `/`.
pub(super) fn tail(base: &str, path: &str) -> String {
    let rest = path.strip_prefix(base).unwrap_or("");
    if rest.is_empty() {
        "/".to_string()
    } else if rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{rest}")
    }
}

/// Drop `x-ferris-capability` before the request reaches the guest.
fn strip_capability_header(mut req: Request<Body>) -> Request<Body> {
    req.headers_mut().remove(CAPABILITY_HEADER);
    req
}

/// Serve the declared routes of `apps` on a standalone router.
///
/// This is the same table `build_router` mounts, exposed so an integration test
/// (or an embedder) can exercise route dispatch — auth, body caps, collision
/// refusal, `%stream` upgrades — without booting the whole Desk. `auth` gates the
/// `session` routes; `client_identity` binding stays with the caller because it
/// depends on the connection's TLS facts.
///
/// An invalid or colliding route is logged and left unmounted, so the returned
/// router simply does not answer that path.
pub fn declared_route_router(
    plugins: Option<std::sync::Arc<PluginStore>>,
    auth: ferris_auth::AuthState,
    apps: &[(String, NetworkSpec)],
    mode: RouteMode,
) -> Router {
    let state = DeclaredRouteState {
        plugins,
        auth: auth.clone(),
    };
    build::<DeclaredRouteState>(apps, mode)
        .layer(axum::middleware::from_fn_with_state(
            auth,
            crate::router::plugin_http::bind_plugin_user,
        ))
        .layer(axum::middleware::from_fn(
            crate::client_identity::bind_client_identity,
        ))
        .with_state(state)
}
