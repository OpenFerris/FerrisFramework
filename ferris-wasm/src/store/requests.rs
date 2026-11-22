//! Request, stream, job and UI-asset entry points into a registered app.

use anyhow::Result;
use tracing::warn;

use ferris_auth::User;

use super::registry::PluginStore;
use crate::{HttpRequest, Response, StreamEvent, StreamInfo};

impl PluginStore {
    /// Forward an HTTP request to the loaded plugin (dev-only route), checking
    /// the requested name matches the loaded plugin.
    ///
    /// Route an HTTP request into the app. Returns the full response
    /// (status, headers, body bytes).
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the requested name does not
    /// match the loaded plugin, or the guest rejects the request.
    pub fn handle_request(&self, name: &str, req: &HttpRequest) -> Result<Response> {
        self.handle_request_as(name, req, None)
    }

    /// Like [`handle_request`](Self::handle_request), but binds `user` as the
    /// ambient identity for the call, so the plugin's `session` imports report
    /// the caller instead of a guest. `None` is anonymous — the previous
    /// dev-mode behaviour. The identity is scoped to the call by
    /// [`crate::Plugin::handle_request_for_route`].
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the requested name does not
    /// match the loaded plugin, or the guest rejects the request.
    pub fn handle_request_as(
        &self,
        name: &str,
        req: &HttpRequest,
        user: Option<User>,
    ) -> Result<Response> {
        self.handle_request_for_route(name, crate::PLUGIN_ROUTE_ID, req, user)
    }

    /// Route an HTTP request into the app naming the matched route, so the
    /// guest receives the `route-id` (`route.id`, `plugins` for the
    /// app-wide prefix, or `method:<name>`).
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the requested name does not
    /// match the loaded plugin, or the guest rejects the request.
    pub fn handle_request_for_route(
        &self,
        name: &str,
        route_id: &str,
        req: &HttpRequest,
        user: Option<User>,
    ) -> Result<Response> {
        let Some(plugin) = self.get(name) else {
            warn!(requested = %name, "plugin request refused: no app is registered under that name");
            anyhow::bail!("plugin `{name}` is not loaded");
        };
        let res = plugin.handle_request_for_route(route_id, req, user);
        self.guard_trap(res, name).map_err(anyhow::Error::from)
    }

    /// Route a declared API method into the loaded app's `handle-request` under
    /// the reserved `method:<name>` route id.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the requested name does not
    /// match the loaded plugin, or the guest rejects the request.
    pub fn handle_method(
        &self,
        name: &str,
        method: &str,
        req: &HttpRequest,
        user: Option<User>,
    ) -> Result<Response> {
        let Some(plugin) = self.get(name) else {
            anyhow::bail!("plugin `{name}` is not loaded");
        };
        let res = plugin.handle_method(method, req, user);
        self.guard_trap(res, name).map_err(anyhow::Error::from)
    }

    /// Announce a live stream (an accepted raw WebSocket, a gRPC/TCP
    /// connection the host forwards) to the loaded plugin, so its `streams::info`
    /// import sees the transport metadata. The requested name must match the
    /// loaded plugin.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded or the requested name does
    /// not match the loaded plugin.
    pub fn register_stream(&self, name: &str, stream_id: &str, info: StreamInfo) -> Result<()> {
        let Some(plugin) = self.get(name) else {
            anyhow::bail!("plugin `{name}` is not loaded");
        };
        plugin.register_stream(stream_id, info);
        Ok(())
    }

    /// Forget a closed stream. EOF is *not* delivered here: the caller sends an
    /// empty chunk to `handle_stream` first, then unregisters, so the plugin
    /// observes the close in the order the connection actually closed.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded or the requested name does
    /// not match the loaded plugin.
    pub fn unregister_stream(&self, name: &str, stream_id: &str) -> Result<()> {
        let Some(plugin) = self.get(name) else {
            anyhow::bail!("plugin `{name}` is not loaded");
        };
        plugin.unregister_stream(stream_id);
        Ok(())
    }

    /// Forward one chunk of a live stream to the loaded plugin's
    /// `handle-stream` export and return the bytes to relay back (empty =
    /// ack/drop). Chunks are data only: open/close travel through
    /// [`handle_stream_event`](Self::handle_stream_event).
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the requested name does not
    /// match the loaded plugin, or the guest rejects the chunk.
    pub fn handle_stream(&self, name: &str, stream_id: &str, chunk: &[u8]) -> Result<Vec<u8>> {
        self.handle_stream_as(name, stream_id, chunk, None)
    }

    /// Like [`handle_stream`](Self::handle_stream), but binds `user` as the
    /// ambient identity for the call.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the requested name does not
    /// match the loaded plugin, or the guest rejects the chunk.
    pub fn handle_stream_as(
        &self,
        name: &str,
        stream_id: &str,
        chunk: &[u8],
        user: Option<User>,
    ) -> Result<Vec<u8>> {
        let Some(plugin) = self.get(name) else {
            anyhow::bail!("plugin `{name}` is not loaded");
        };
        let res = plugin.handle_stream_as(stream_id, chunk, user);
        self.guard_trap(res, name).map_err(anyhow::Error::from)
    }

    /// Forward one inbound datagram from a UDP, raw or ICMP socket to the
    /// loaded plugin's `handle-datagram` export and return the bytes to send
    /// back to `peer` (empty drops the datagram). `stream_id` is the socket,
    /// not a peer connection; `peer` is the sender.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the requested name does not
    /// match the loaded plugin, or the guest rejects the datagram.
    pub fn handle_datagram(
        &self,
        name: &str,
        stream_id: &str,
        data: &[u8],
        peer: &str,
    ) -> Result<Vec<u8>> {
        self.handle_datagram_as(name, stream_id, data, peer, None)
    }

    /// Like [`handle_datagram`](Self::handle_datagram), but binds `user` as the
    /// ambient identity for the call.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the requested name does not
    /// match the loaded plugin, or the guest rejects the datagram.
    pub fn handle_datagram_as(
        &self,
        name: &str,
        stream_id: &str,
        data: &[u8],
        peer: &str,
        user: Option<User>,
    ) -> Result<Vec<u8>> {
        let Some(plugin) = self.get(name) else {
            anyhow::bail!("plugin `{name}` is not loaded");
        };
        let res = plugin.handle_datagram_as(stream_id, data, peer, user);
        self.guard_trap(res, name).map_err(anyhow::Error::from)
    }

    /// Deliver one stream lifecycle event (`opened`, `closed`, `error`) to the
    /// loaded plugin's `handle-stream-event` export.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the requested name does not
    /// match the loaded plugin, or the guest rejects the event.
    pub fn handle_stream_event(
        &self,
        name: &str,
        stream_id: &str,
        event: StreamEvent,
    ) -> Result<()> {
        self.handle_stream_event_as(name, stream_id, event, None)
    }

    /// Like [`handle_stream_event`](Self::handle_stream_event), but binds
    /// `user` as the ambient identity for the call.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the requested name does not
    /// match the loaded plugin, or the guest rejects the event.
    pub fn handle_stream_event_as(
        &self,
        name: &str,
        stream_id: &str,
        event: StreamEvent,
        user: Option<User>,
    ) -> Result<()> {
        let Some(plugin) = self.get(name) else {
            anyhow::bail!("plugin `{name}` is not loaded");
        };
        let res = plugin.on_stream_event_as(stream_id, event, user);
        self.guard_trap(res, name).map_err(anyhow::Error::from)
    }

    /// Route one claimed background job into the `handle-job` export of the app
    /// that owns its `job_type`, carrying the type and the JSON `payload` the
    /// enqueue wrote.
    ///
    /// The owner is resolved from the job type, never from
    /// [`PluginStore::loaded`]: with N apps the last-loaded one used to run
    /// every app's jobs, so a schedule of app A executed B's code (or failed
    /// there) and any app could claim any type it liked through
    /// `platform::enqueue-job` (WASM-007/SEC-11). See
    /// [`PluginStore::owner_of_job`] for the naming rule.
    ///
    /// There is **no fallback**: with no app loaded, or no app owning the type,
    /// the job fails loudly (the worker retries, then dead-letters) instead of
    /// being silently dropped or handed to an arbitrary app. The server's app
    /// worker registers this behind its job dispatcher so a `job_name` a
    /// schedule enqueues actually reaches the guest — before this, a cron could
    /// be registered but nothing ever ran the app's code for it.
    ///
    /// # Errors
    ///
    /// Returns a typed `not-found` plugin error when no app owns `job_type`, and
    /// the guest's own rejection when the owning app fails the job.
    pub fn handle_job(&self, job_type: &str, payload: &str) -> Result<String> {
        self.handle_job_as(job_type, payload, None)
    }

    /// Like [`handle_job`](Self::handle_job), but binds `user` as the ambient
    /// identity for the call (`None` is anonymous).
    ///
    /// # Errors
    ///
    /// Returns a typed `not-found` plugin error when no app owns `job_type`, and
    /// the guest's own rejection when the owning app fails the job.
    pub fn handle_job_as(
        &self,
        job_type: &str,
        payload: &str,
        user: Option<User>,
    ) -> Result<String> {
        // Keep the empty-store message the worker (and its retry/dead-letter
        // policy) has always seen: nothing is loaded, nothing can own the job.
        if self.plugins().is_empty() {
            anyhow::bail!("no plugin loaded");
        }
        let Some(owner) = self.owner_of_job(job_type) else {
            if self.notice_once(format!("job:{job_type}")) {
                warn!(
                    job_type,
                    "no loaded app owns this job type; refused (name app jobs `<app>.<job>`)"
                );
            }
            return Err(job_not_found(
                "-",
                format!("no loaded app owns job type `{job_type}`; name app jobs `<app>.<job>`"),
            ));
        };
        let Some(plugin) = self.get(&owner) else {
            return Err(job_not_found(
                &owner,
                format!("app `{owner}` owns job type `{job_type}` but is not registered"),
            ));
        };
        let res = plugin.on_job(job_type, payload, user);
        self.guard_trap(res, &owner).map_err(anyhow::Error::from)
    }

    /// Serve one UI asset (script, stylesheet, image, …) of the loaded
    /// plugin's `ui-asset` export to the Desk, for `/plugins/{name}/asset/{path}`.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the requested name does not
    /// match the loaded plugin, or the guest rejects the path.
    pub fn ui_asset(&self, name: &str, path: &str) -> Result<Vec<u8>> {
        let Some(plugin) = self.get(name) else {
            anyhow::bail!("plugin `{name}` is not loaded");
        };
        let res = plugin.ui_asset(path);
        self.guard_trap(res, name).map_err(anyhow::Error::from)
    }

    /// SSR markup for one of the loaded plugin's widget definitions, for
    /// `GET /plugins/{name}/widget/{widget}?params=…`. `params` is the
    /// instance's (name, value) pairs — the same values the mounted element
    /// receives in the DOM — so server HTML and client hydration agree.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the requested name does not
    /// match the loaded plugin, or the guest rejects the widget reference.
    pub fn ui_widget(
        &self,
        name: &str,
        widget: &str,
        params: &[(String, String)],
    ) -> Result<String> {
        self.ui_widget_as(name, widget, params, None)
    }

    /// Like [`ui_widget`](Self::ui_widget), but binds `user` as the ambient
    /// identity for the call, so SSR that reads documents is authorized as the
    /// real caller. `None` is anonymous.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the requested name does not
    /// match the loaded plugin, or the guest rejects the widget reference.
    pub fn ui_widget_as(
        &self,
        name: &str,
        widget: &str,
        params: &[(String, String)],
        user: Option<User>,
    ) -> Result<String> {
        let Some(plugin) = self.get(name) else {
            anyhow::bail!("plugin `{name}` is not loaded");
        };
        let res = plugin.ui_widget_as(widget, params, user);
        self.guard_trap(res, name).map_err(anyhow::Error::from)
    }
}

/// Typed refusal for a job that belongs to no loaded app.
///
/// `NotFound` (404) classifies it as "there is no handler for this type", which
/// the worker can tell apart from a guest failure without parsing text; the
/// message names the naming rule so the fix is mechanical.
fn job_not_found(owner: &str, message: String) -> anyhow::Error {
    anyhow::Error::from(crate::WasmError::PluginError {
        name: owner.to_string(),
        call: "handle-job",
        kind: crate::PluginErrorKind::NotFound,
        message,
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::PluginStore;
    use crate::runtime::{RuntimeConfig, SharedRuntime};

    async fn store() -> PluginStore {
        let runtime = Arc::new(
            SharedRuntime::new(RuntimeConfig {
                enable_epoch_interrupts: false,
                ..Default::default()
            })
            .expect("SharedRuntime"),
        );
        PluginStore::new(runtime, ferris_db::memory_db().await.expect("memory db"))
    }

    fn fixture(dir: &str, file: &str) -> Option<PathBuf> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "tests/fixtures/{dir}/target/wasm32-wasip2/debug/{file}.wasm"
        ));
        if path.exists() {
            Some(path)
        } else {
            eprintln!(
                "SKIPPED: cargo build --target wasm32-wasip2 \
                 --manifest-path ferris-wasm/tests/fixtures/{dir}/Cargo.toml"
            );
            None
        }
    }

    /// WIT `handle-job` on `probe-ok` echoes `"{job-type}|{payload}"`; the trap
    /// probe answers an empty string, so the result tells the two apps apart.
    ///
    /// WASM-007/SEC-11: a job runs in the app that owns its type, not in the
    /// app that happened to be loaded last.
    #[tokio::test]
    async fn a_job_runs_only_in_the_app_that_owns_its_type() {
        let (Some(ok), Some(trap)) = (
            fixture("probe-ok", "probe_ok"),
            fixture("probe-trap", "probe_trap"),
        ) else {
            return;
        };
        let store = store().await;
        store.swap_load_with_name(&ok, "a").expect("load a");
        store.swap_load_with_name(&trap, "b").expect("load b");
        assert_eq!(store.name().as_deref(), Some("b"), "b is the loaded app");

        let out = store
            .handle_job("a.nightly", "{\"doctype\":\"Task\"}")
            .expect("the owner runs the job");
        assert_eq!(
            out, "a.nightly|{\"doctype\":\"Task\"}",
            "the job was not routed to app `a`"
        );
    }

    /// A type no loaded app owns is refused with a typed `not-found`, never
    /// handed to an arbitrary app.
    #[tokio::test]
    async fn a_job_type_no_app_owns_is_refused() {
        let (Some(ok), Some(trap)) = (
            fixture("probe-ok", "probe_ok"),
            fixture("probe-trap", "probe_trap"),
        ) else {
            return;
        };
        let store = store().await;
        store.swap_load_with_name(&ok, "a").expect("load a");
        store.swap_load_with_name(&trap, "b").expect("load b");

        for job_type in ["c.nightly", "task_nightly"] {
            let err = store
                .handle_job(job_type, "{}")
                .expect_err("two apps make an unprefixed/foreign type ambiguous");
            assert!(
                matches!(
                    err.downcast_ref::<crate::WasmError>(),
                    Some(crate::WasmError::PluginError {
                        kind: crate::PluginErrorKind::NotFound,
                        ..
                    })
                ),
                "expected a typed not-found, got {err:?}"
            );
        }
    }

    /// One app is unambiguous: every job type it enqueues is its own, which is
    /// the behaviour a single-app install (and the scaffold) relies on.
    #[tokio::test]
    async fn a_single_app_owns_unprefixed_job_types() {
        let Some(ok) = fixture("probe-ok", "probe_ok") else {
            return;
        };
        let store = store().await;
        store.swap_load_with_name(&ok, "probe").expect("load probe");
        assert_eq!(store.owner_of_job("task_nightly").as_deref(), Some("probe"));
        let out = store
            .handle_job("task_nightly", "{}")
            .expect("the only app runs it");
        assert_eq!(out, "task_nightly|{}");
    }

    /// An empty store keeps its loud message for the worker's retry policy.
    #[tokio::test]
    async fn an_empty_store_still_refuses_jobs_loudly() {
        let store = store().await;
        let err = store
            .handle_job("a.nightly", "{}")
            .expect_err("no app can own the job");
        assert!(format!("{err}").contains("no plugin loaded"), "{err}");
    }
}
