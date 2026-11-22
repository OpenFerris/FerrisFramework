//! The app-plugin twin of `ferris-server`: boot load / hot swap, the
//! `/plugins/*` HTTP routes and the `/api/method/<name>` bridge.

use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{ConnectInfo, FromRequestParts, Path as AxumPath, Query, Request, State},
    http::StatusCode,
    http::header::CONTENT_TYPE,
    middleware,
    response::{IntoResponse, Response},
};
use ferris_api::{ApiState, MethodHandler};
use ferris_auth::middleware::OptionalUser;
use ferris_auth::{AuthState, User};
use ferris_wasm::{
    HttpRequest, PluginErrorKind, PluginStore, Response as PluginResponse, WasmError,
    load_component, sanitize_response_headers,
};
use tracing::{info, warn};

use super::state::{AppWasm, DevState, PLUGIN_BODY_LIMIT_BYTES};

/// Loads the project's app plugin (forwarded by the supervisor via env vars) so
/// the Desk lists its modules. Missing/invalid plugins degrade to "no app".
pub(super) async fn load_app_plugin(plugins: &Arc<PluginStore>) -> Result<Option<AppWasm>> {
    let (Some(name), Some(core_wasm)) = (
        env::var("FERRIS_DEV_APP_NAME").ok(),
        env::var("FERRIS_DEV_APP_CORE_WASM").ok(),
    ) else {
        return Ok(None);
    };
    let core_wasm = PathBuf::from(core_wasm);
    if !core_wasm.is_file() {
        warn!(
            "app wasm {} not found; the Desk will run without app modules",
            core_wasm.display()
        );
        return Ok(None);
    }
    let app = AppWasm { name, core_wasm };
    match swap_plugin(plugins.clone(), app.clone()) {
        Ok(()) => {
            info!(plugin = %plugins.name().unwrap_or_default(), "app plugin loaded into the dev server");
            Ok(Some(app))
        }
        Err(e) => {
            warn!(error = %e, "app plugin could not be loaded; the Desk will run without app modules");
            Ok(None)
        }
    }
}

/// Hot-swaps the app wasm into the running dev server. Used by the initial load
/// and by the loopback reload endpoint (`--hot-reload both`).
///
/// Runs on a dedicated plain thread: the plugin host's setup work
/// (`RealHostState`) blocks on a tokio runtime, which must not be driven from
/// inside the `#[tokio::main]` runtime. The thread gives the load its own
/// multi-threaded runtime so guest `describe()` host imports keep a reactor.
pub(super) fn swap_plugin(plugins: Arc<PluginStore>, app: AppWasm) -> Result<()> {
    std::thread::scope(|s| {
        let handle = s.spawn(move || {
            // Wiring calls the guest's `describe()` export, and a plugin may run
            // host imports from there (e.g. `scheduler.list-schedules` to make
            // cron registration idempotent). Those imports resolve through
            // `tokio::runtime::Handle::current().block_on` in `ferris-wasm`,
            // which panics with "there is no reactor running" unless the
            // calling thread is inside a multi-threaded runtime with the worker
            // parked — `block_in_place` is what the request path uses too.
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let component = load_component(&app.core_wasm)?;
            rt.block_on(async {
                tokio::task::block_in_place(|| plugins.swap_load_and_wire(&component, &app.name))
            })?;
            Ok(())
        });
        handle
            .join()
            .unwrap_or_else(|_| anyhow::bail!("app plugin loader thread panicked"))
    })
}

/// `POST /__dev/reload-app?token=...` — rebuild+swap the app plugin. Loopback
/// only and token-gated (the supervisor shares a fresh UUID via env).
pub(super) async fn reload_app(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<HashMap<String, String>>,
    State(state): State<DevState>,
) -> Response {
    if !addr.ip().is_loopback() {
        return (StatusCode::FORBIDDEN, "loopback only").into_response();
    }
    if state.reload.token.is_empty()
        || query.get("token").map(String::as_str) != Some(state.reload.token.as_str())
    {
        return (StatusCode::FORBIDDEN, "missing or invalid reload token").into_response();
    }
    let Some(app) = state.reload.app.clone() else {
        return (StatusCode::BAD_REQUEST, "no app plugin configured").into_response();
    };
    let plugin_name = app.name.clone();
    match swap_plugin(state.plugins.clone(), app) {
        Ok(()) => {
            // The new build may declare different API methods: re-run the
            // boot-time registration (same-name entries are replaced).
            register_plugin_methods(&state.api, &state.plugins);
            Json(serde_json::json!({ "status": "reloaded", "plugin": plugin_name })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("app plugin reload failed: {e}"),
        )
            .into_response(),
    }
}

/// Ambient identity bound to plugin HTTP requests. Resolved from the desk
/// session (bearer token or cookie) before the plugin runs; `None` means the
/// caller was anonymous, which the plugin's `session` import reports as a
/// guest rather than a fabricated identity — the `ferris-server` twin.
#[derive(Clone)]
pub(super) struct PluginUser(Option<User>);

/// Resolve the optional session user for plugin routes and stash it in request
/// extensions, without rejecting anonymous callers. Plugins keep working for
/// public endpoints, but `session::current` inside a logged-in request now
/// returns the real user (id/email/roles) instead of always a guest.
pub(super) async fn bind_plugin_user(
    State(auth): State<AuthState>,
    request: Request,
    next: middleware::Next,
) -> Result<Response, Response> {
    let (mut parts, body) = request.into_parts();
    let user = OptionalUser::from_request_parts(&mut parts, &auth)
        .await
        .expect("OptionalUser has an infallible rejection")
        .0;
    parts.extensions.insert(PluginUser(user));
    let request = Request::from_parts(parts, body);
    Ok(next.run(request).await)
}

pub(super) async fn plugin_describe(
    State(plugins): State<Arc<PluginStore>>,
    AxumPath(name): AxumPath<String>,
) -> Result<Response, StatusCode> {
    if let Some(loaded) = plugins.name() {
        if loaded != name {
            return Err(StatusCode::NOT_FOUND);
        }
    }
    // The wasm call is blocking and its host imports need a tokio context, so
    // park this worker out of the scheduler while it runs. `describe_json`
    // projects the typed WIT `descriptor` record onto the stable JSON surface
    // `/plugins/{name}` has served since 1.x.
    let desc = tokio::task::block_in_place(move || plugins.describe_json());
    match desc {
        Ok(desc) => Ok(Response::builder()
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(desc))
            .expect("static response headers are always valid")),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

pub(super) async fn plugin_handle_request(
    State(plugins): State<Arc<PluginStore>>,
    AxumPath((name, rest)): AxumPath<(String, String)>,
    req: Request<Body>,
) -> Result<Response, StatusCode> {
    // Ambient session user resolved by `bind_plugin_user` (`None` when the
    // caller was anonymous) — forwarded so `session::current` inside the guest
    // reports the real caller instead of always a guest.
    let user = req
        .extensions()
        .get::<PluginUser>()
        .and_then(|pu| pu.0.clone());

    let path = if rest.is_empty() {
        "/".to_string()
    } else {
        format!("/{rest}")
    };
    let request = to_wit_request(path, req).await?;

    let outcome =
        tokio::task::block_in_place(move || plugins.handle_request_as(&name, &request, user));

    match outcome {
        Ok(reply) => Ok(plugin_response(reply)),
        Err(e) => {
            // Same mapping as `ferris-server`'s router: the plugin's typed
            // `WasmError` carries its semantic status (404/403/422/409/429,
            // trap → 500); wiring guards → 404; anything else → 400.
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

/// Build the host response from the plugin's typed `http.response` — the
/// `ferris-server` twin (`plugin_response`): status/body verbatim, headers
/// after `sanitize_response_headers`, `text/plain` only when the plugin set
/// no `content-type`.
fn plugin_response(resp: PluginResponse) -> Response {
    let headers = sanitize_response_headers(&resp.headers);
    let status = StatusCode::from_u16(resp.status).unwrap_or_else(|_| {
        warn!(
            status = resp.status,
            "plugin returned an invalid status; answering 500"
        );
        StatusCode::INTERNAL_SERVER_ERROR
    });
    let mut builder = Response::builder().status(status);
    if !headers.iter().any(|(name, _)| name == "content-type") {
        builder = builder.header(CONTENT_TYPE, "text/plain; charset=utf-8");
    }
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    builder
        .body(Body::from(resp.body))
        .expect("sanitize_response_headers validated every header")
}

/// Project an axum request into the typed `HttpRequest` the guest sees — the
/// `ferris-server` twin (`to_wit_request`): query without the `?`, header
/// values lossy UTF-8, body as raw bytes (1 MiB cap).
pub(super) async fn to_wit_request(
    path: String,
    req: Request<Body>,
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
    let body = to_bytes(req.into_body(), PLUGIN_BODY_LIMIT_BYTES)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    Ok(HttpRequest {
        method,
        path,
        query,
        headers,
        body: body.to_vec(),
        client_identity: None,
    })
}

/// One `/api/method/<name>` call forwarded to the plugin's `handle-request`
/// export — the `ferris-server` twin (`PluginMethodHandler`). Re-verifies the
/// descriptor on every call, so after a hot reload that dropped the method the
/// call answers 404 instead of reaching a stale route.
#[derive(Clone)]
struct SsrPluginMethodHandler {
    plugins: Arc<PluginStore>,
    plugin: String,
    method: String,
}

#[async_trait]
impl MethodHandler for SsrPluginMethodHandler {
    fn owner(&self) -> Option<&str> {
        Some(&self.plugin)
    }

    async fn call(&self, params: ferris_core::Value) -> ferris_core::Result<ferris_core::Value> {
        let body = serde_json::to_vec(&params).map_err(|e| {
            ferris_core::Error::Validation(ferris_core::ValidationError::Invalid(format!(
                "cannot serialize method params: {e}"
            )))
        })?;
        let route = format!("/api/method/{}", self.method);

        // The API router's `bind_ambient_user` layer scopes the caller
        // thread-locally; capture it before the blocking WASM call.
        let user = ferris_document::current_user();

        let declared = tokio::task::block_in_place(|| self.plugins.methods())
            .map_err(|e| map_plugin_error(&e))?;
        if declared.iter().all(|spec| spec.name != self.method) {
            return Err(ferris_core::Error::Document(
                ferris_core::DocumentError::NotFound {
                    doctype: "plugin method".to_string(),
                    name: self.method.clone(),
                },
            ));
        }

        // The RPC path is always a JSON POST (see the `ferris-server` twin).
        let request = HttpRequest {
            method: "POST".to_string(),
            path: route,
            query: String::new(),
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body,
            client_identity: None,
        };

        let plugin = self.plugin.clone();
        let resp = tokio::task::block_in_place(move || {
            self.plugins
                .handle_method(&plugin, &self.method, &request, user)
                .map_err(|e| map_plugin_error(&e))
        })?;

        if resp.status >= 400 {
            let preview: String = String::from_utf8_lossy(&resp.body)
                .chars()
                .take(512)
                .collect();
            return Err(ferris_core::Error::Validation(
                ferris_core::ValidationError::Invalid(format!(
                    "plugin method `{}` failed with status {}: {preview}",
                    self.method, resp.status
                )),
            ));
        }

        serde_json::from_slice(&resp.body).map_err(|e| {
            ferris_core::Error::Validation(ferris_core::ValidationError::Invalid(format!(
                "plugin method `{}` returned a non-JSON body: {e}",
                self.method
            )))
        })
    }
}

/// Register one `/api/method/<name>` handler per API method the loaded plugin
/// declares in its typed `describe()` descriptor — the `ferris-server` twin
/// (`register_plugin_methods`). Idempotent per name, so it runs again after
/// every hot swap; a plugin that isn't loaded (or whose `describe` fails)
/// registers nothing, exactly like the server.
pub(super) fn register_plugin_methods(api: &ApiState, plugins: &Arc<PluginStore>) {
    let Ok(methods) = tokio::task::block_in_place(|| plugins.methods()) else {
        return;
    };
    for method in methods {
        if method.overrides && !api.methods.contains(&method.name) {
            tracing::error!(
                method = %method.name,
                "declared an `overrides` method with nothing below it; not registered"
            );
            continue;
        }
        api.methods.declare(
            method.name.clone(),
            Arc::new(SsrPluginMethodHandler {
                plugins: Arc::clone(plugins),
                plugin: plugins.name().unwrap_or_default(),
                method: method.name,
            }),
        );
    }
}

/// Translate an error from forwarding a plugin call into the typed
/// `ferris_core::Error` the REST layer maps to a status — the same mapping
/// `ferris-server`'s `map_plugin_error` uses (typed kind → 404/403/400/409/401,
/// traps → 500, wiring guards → 404, anything else → 500).
fn map_plugin_error(err: &anyhow::Error) -> ferris_core::Error {
    if let Some(wasm) = err.downcast_ref::<WasmError>() {
        return match wasm {
            WasmError::PluginError { kind, message, .. } => match kind {
                PluginErrorKind::NotFound => {
                    ferris_core::Error::Document(ferris_core::DocumentError::NotFound {
                        doctype: "plugin method".to_string(),
                        name: message.clone(),
                    })
                }
                PluginErrorKind::PermissionDenied => {
                    ferris_core::Error::Authz(ferris_core::AuthzError::PermissionDenied {
                        action: "plugin method".to_string(),
                        resource: message.clone(),
                    })
                }
                PluginErrorKind::ValidationFailed => ferris_core::Error::Validation(
                    ferris_core::ValidationError::Invalid(message.clone()),
                ),
                PluginErrorKind::Conflict => {
                    ferris_core::Error::Document(ferris_core::DocumentError::AlreadyExists {
                        doctype: "plugin method".to_string(),
                        name: message.clone(),
                    })
                }
                PluginErrorKind::RateLimited(_) => {
                    ferris_core::Error::Auth(ferris_core::AuthError::RateLimited)
                }
                // A plugin call that ran out of time: the job-timeout variant,
                // which `ferris-api` renders as 504 (the same mapping
                // `ferris-server::router::plugins` uses).
                PluginErrorKind::Timeout => ferris_core::Error::Job(ferris_core::JobError::Timeout),
                PluginErrorKind::Internal => {
                    ferris_core::Error::Job(ferris_core::JobError::Failed(message.clone()))
                }
            },
            WasmError::Trap { .. } => {
                ferris_core::Error::Job(ferris_core::JobError::Failed(err.to_string()))
            }
            _ => ferris_core::Error::Document(ferris_core::DocumentError::NotFound {
                doctype: "plugin method".to_string(),
                name: err.to_string(),
            }),
        };
    }
    let msg = err.to_string();
    if msg.contains("not loaded") || msg.contains("mismatch") {
        ferris_core::Error::Document(ferris_core::DocumentError::NotFound {
            doctype: "plugin method".to_string(),
            name: msg,
        })
    } else {
        ferris_core::Error::Job(ferris_core::JobError::Failed(msg))
    }
}
