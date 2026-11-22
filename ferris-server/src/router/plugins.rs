//! WASM plugin plumbing: the blocking-call guard rails and the RPC method
//! handlers registered from each app's descriptor.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use ferris_api::{ApiState, MethodHandler};
use ferris_core::Value;
use ferris_wasm::{HttpRequest, PluginErrorKind, PluginStore, WasmError};

use crate::client_identity::current_client_identity;

/// Hard cap on the request body we hand to a WASM plugin. Kept below the
/// global `body_limit_mb` on purpose: plugin handlers run inside
/// `block_in_place`, which parks a tokio worker thread for the duration of
/// the WASM call — we don't want a giant request pinning a worker.
pub(super) const PLUGIN_BODY_LIMIT_BYTES: usize = 1024 * 1024;

/// Ceiling on concurrent blocking guest calls (SRV6).
///
/// Every call parks a tokio worker for its whole duration, and `block_in_place`
/// is not cancellable: when `TimeoutLayer` returns a 504 the guest keeps running
/// and the thread stays parked until the WASM epoch budget traps it. Sizing the
/// ceiling to the worker count keeps a slow or hostile guest degrading to
/// itself instead of starving the API's worker pool.
static PLUGIN_CALLS: LazyLock<tokio::sync::Semaphore> = LazyLock::new(|| {
    let permits = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(4);
    tokio::sync::Semaphore::new(permits)
});

/// Run a blocking plugin call from an async handler within [`PLUGIN_CALLS`].
///
/// The semaphore is process-static and never closed, so `acquire` cannot fail;
/// were it ever to, the call still runs (unbounded) rather than panicking the
/// request path.
pub(super) async fn run_plugin<T>(operation: impl FnOnce() -> T) -> T {
    let _permit = PLUGIN_CALLS.acquire().await;
    run_blocking(operation)
}

/// Run a blocking plugin call from a synchronous context (boot-time method
/// registration).
///
/// `block_in_place` exists only on a multi-thread runtime; on a current-thread
/// runtime (a `#[tokio::test]`, an embedded host) it panics, which is what made
/// the public `build_router` unusable there. Fall back to running the closure
/// inline: it blocks the reactor for its duration, which is the honest cost,
/// instead of taking the process down (SRV15).
pub(super) fn run_blocking<T>(operation: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(operation)
        }
        _ => operation(),
    }
}

/// Register one [`MethodHandler`] per API method **every** loaded app declares
/// in its typed `describe()` descriptor.
///
/// Multi-app installs are the reason this iterates the registry instead of the
/// most recent swap: each app's methods resolve to that app (SRV17).
///
/// Each handler forwards `/api/method/<name>` to the plugin's `handle-request`
/// export at the reserved `/api/method/<name>` route, authorized as the
/// authenticated caller. Registration happens once at boot: the handler itself
/// re-verifies the descriptor on every call so a hot reload that drops a
/// method answers `404` instead of dispatching to a stale export.
pub(super) fn register_plugin_methods(api: &ApiState, plugins: &Arc<PluginStore>) {
    for plugin in plugins.plugins() {
        let name = plugin.name().to_string();
        let Ok(methods) = run_blocking(|| plugins.methods_for(&name)) else {
            // This app's `describe()` failed: skip it, keep the others.
            continue;
        };
        for method in methods {
            // An override must have something to shadow: a typo would otherwise
            // publish a method that only looks like it replaces a framework API.
            if method.overrides && !api.methods.contains(&method.name) {
                tracing::error!(
                    app = %name,
                    method = %method.name,
                    "declared an `overrides` method with nothing below it; not registered"
                );
                continue;
            }
            api.methods.declare(
                method.name.clone(),
                Arc::new(PluginMethodHandler {
                    plugins: Arc::clone(plugins),
                    plugin: name.clone(),
                    method: method.name.clone(),
                }),
            );
            if method.overrides {
                // One line per inherited API: an operator can see which app
                // shadows a framework method without reading the config.
                tracing::info!(
                    method = %method.name,
                    chain = ?api.methods.owners(&method.name),
                    "app method overrides an existing API"
                );
            }
        }
    }
}

/// `MethodHandler` forwarding a framework `/api/method/<name>` call to the
/// loaded WASM plugin's `handle-request` export at the reserved
/// `/api/method/<name>` route, bound to the authenticated caller.
#[derive(Clone)]
struct PluginMethodHandler {
    plugins: Arc<PluginStore>,
    plugin: String,
    method: String,
}

#[async_trait]
impl MethodHandler for PluginMethodHandler {
    fn owner(&self) -> Option<&str> {
        Some(&self.plugin)
    }

    async fn call(&self, params: Value) -> ferris_core::Result<Value> {
        let body = serde_json::to_vec(&params).map_err(|e| {
            ferris_core::Error::Validation(ferris_core::ValidationError::Invalid(format!(
                "cannot serialize method params: {e}"
            )))
        })?;
        let route = format!("/api/method/{}", self.method);

        // The authenticated caller's identity is scoped thread-locally by the
        // API router's `bind_ambient_user` layer; capture it before the
        // blocking WASM call so the plugin's `session` import reports the real
        // caller (the same user `session.can` and host docstore calls check).
        let user = ferris_document::current_user();

        // The verified client-certificate identity (mTLS) is scoped as a
        // task-local by `bind_client_identity`; like the user it must be
        // captured before `block_in_place`, where task-locals are not visible.
        let client_identity = current_client_identity();

        // Re-verify the descriptor on every call: after a hot reload the plugin
        // may have dropped/renamed the method, and a stale registration must
        // answer `not-found` instead of dispatching blindly.
        let declared = run_plugin(|| self.plugins.methods_for(&self.plugin))
            .await
            .map_err(|e| map_plugin_error(&e))?;
        if declared.iter().all(|spec| spec.name != self.method) {
            return Err(ferris_core::Error::Document(
                ferris_core::DocumentError::NotFound {
                    doctype: "plugin method".to_string(),
                    name: self.method.clone(),
                },
            ));
        }

        // The RPC path is always a JSON POST; the guest sees the same shape a
        // REST caller would (method/query/headers were invisible in 1.0).
        let request = HttpRequest {
            method: "POST".to_string(),
            path: route,
            query: String::new(),
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body,
            client_identity: client_identity.map(ferris_wasm::wit_client_identity),
        };

        // `method:<name>` is the route id: a guest that dispatches on it
        // must not fall through to its path matcher (the old `plugins` id made
        // every declared method an `unknown route`).
        let plugin = self.plugin.clone();
        let method = self.method.clone();
        let resp = run_plugin(move || {
            self.plugins
                .handle_method(&plugin, &method, &request, user)
                .map_err(|e| map_plugin_error(&e))
        })
        .await?;

        // 1.0 had no status to honour (the body was the whole reply). A 4xx/5xx
        // from the guest is now a failure of the method call, not a JSON result
        // to hand back as `Value`.
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

/// Translate an error from forwarding a plugin call into the typed
/// [`ferris_core::Error`] the REST layer's `error_response` maps to a status:
/// `WasmError::PluginError` keeps its typed kind (NotFound → 404,
/// PermissionDenied → 403, ValidationFailed → 400, Conflict → 409,
/// RateLimited → 401), traps are internal failures, and wiring errors mean the
/// plugin could not serve the method (404).
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
                // 504: the guest (or the deadline around it) reported that the
                // call did not complete in time. It maps to the job-timeout
                // variant, which `ferris-api` renders as `504 Gateway Timeout`
                // instead of the generic 500 a plain internal error gets.
                PluginErrorKind::Timeout => ferris_core::Error::Job(ferris_core::JobError::Timeout),
                PluginErrorKind::Internal => {
                    ferris_core::Error::Job(ferris_core::JobError::Failed(message.clone()))
                }
            },
            // A trap is the guest crashing the call; nothing the caller can
            // retry.
            WasmError::Trap { .. } => {
                ferris_core::Error::Job(ferris_core::JobError::Failed(err.to_string()))
            }
            _ => ferris_core::Error::Document(ferris_core::DocumentError::NotFound {
                doctype: "plugin method".to_string(),
                name: err.to_string(),
            }),
        };
    }
    // Wiring errors from the guard rails of `handle_request_as`: the plugin is
    // not loaded, or its name no longer matches the requested one. Those mean
    // "this method cannot be served right now", not a client mistake.
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
