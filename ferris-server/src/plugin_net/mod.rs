//! The sockets an app's declaration asks the host to own.
//!
//! An app declares routes, listeners and TLS material; the guest
//! never performs a handshake or binds a port. This module is the half that
//! executes the declaration:
//!
//! * [`streams`] implements [`StreamService`](ferris_wasm::StreamService) —
//!   outbound and inbound byte streams (`tcp`/`tls` and the `ws`/`wss` client),
//!   their lifecycle events and the backpressure between a socket and
//!   `handle-stream`;
//! * [`tls_routes`] serves the `http` routes that carry a `tls-server` record on
//!   a dedicated TLS listener, with the app's certificate and client-auth policy;
//! * [`egress`] is the outbound allow-list the streams share with the HTTP path;
//! * [`material`] is the one place a `tls-material.secret` is resolved against
//!   the app's config store (and its `secrets` grant).
//!
//! Everything here fails loudly. A stream that cannot be opened, a listener
//! that cannot be bound and a certificate that cannot be loaded are typed
//! errors, never an `Ok` that pretends the surface exists.

pub(crate) mod egress;
pub(crate) mod material;
pub(crate) mod streams;
pub mod tls_routes;

pub use streams::{ServerStreams, bind_declared_listeners};

use std::sync::Arc;

use ferris_auth::AuthState;
use ferris_wasm::PluginStore;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::router::plugin_routes::{self, DeclaredRouteState};
use crate::server::ServerSettings;

/// The sockets, listeners and TLS endpoints the loaded apps declared.
///
/// Built once at boot, after the apps are loaded and before the main listener
/// accepts. Dropping it stops every listener it started: the whole surface is
/// owned by this value, so a shutdown cannot leave a plugin port bound.
#[derive(Default)]
pub struct PluginNetwork {
    /// The registered stream transport, when a store was published.
    pub streams: Option<Arc<ServerStreams>>,
    /// Listener tasks (TLS routes and declared `%stream` listeners).
    handles: Vec<JoinHandle<()>>,
}

impl Drop for PluginNetwork {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.abort();
        }
    }
}

impl PluginNetwork {
    /// Register the stream transport and bind every declared listener.
    ///
    /// Every failure is logged and the rest of the surface keeps working: one
    /// app's unbindable port must not deny service to the Desk. The failures are
    /// deliberately **not** silent — a listener the operator configured and did
    /// not get is exactly what this log line is for.
    pub async fn start(
        settings: &ServerSettings,
        plugins: &Arc<PluginStore>,
        auth: &AuthState,
    ) -> Self {
        let transport = Arc::new(ServerStreams::new(Arc::clone(plugins)));
        if !ferris_wasm::set_stream_service(transport.clone() as Arc<dyn ferris_wasm::StreamService>)
        {
            warn!(
                "a stream service is already registered in this process; \
                 `io::connect-stream`/`io::listen-stream` keep using the first one"
            );
            return Self::default();
        }

        match ferris_wasm::set_process_service(std::sync::Arc::new(
            crate::plugin_process::ProcessSupervisor::new(Arc::clone(plugins)),
        )) {
            true => info!("app process supervisor registered"),
            false => warn!(
                "a process supervisor is already registered; `platform::process-*` keeps                  using the first one"
            ),
        }

        let declarations = plugin_routes::collect(plugins);
        let failures = streams::bind_declared_listeners(&transport, &declarations).await;
        for (app, route, error) in failures {
            warn!(app = %app, route = %route, error = %error, "declared stream listener unavailable");
        }

        let state = DeclaredRouteState {
            plugins: Some(Arc::clone(plugins)),
            auth: auth.clone(),
        };
        let mut network = Self {
            streams: Some(transport),
            handles: Vec::new(),
        };
        match tls_routes::plan(plugins, &declarations, |app| {
            settings.config.apps.tls_port_for(app)
        })
        .await
        {
            Ok(plans) if plans.is_empty() => {}
            Ok(plans) => {
                for plan in plans {
                    match tls_routes::spawn(plan, state.clone(), &declarations).await {
                        Ok(handle) => network.handles.push(handle),
                        Err(e) => warn!(error = %e, "plugin TLS listener unavailable"),
                    }
                }
            }
            Err(e) => warn!(error = %e, "plugin TLS listeners could not be planned"),
        }
        info!(
            listeners = network.handles.len(),
            live_streams = network.streams.as_ref().map_or(0, |streams| streams.live()),
            "app network surface started"
        );
        network
    }
}

/// Run one blocking guest call off the reactor.
///
/// The WIT imports are synchronous and the component runs on a `wasmtime::Store`
/// behind a mutex: calling one from an async task would park the reactor for the
/// duration of the guest's work. `spawn_blocking` gives it a dedicated thread on
/// every runtime flavour (a test's current-thread runtime included), which
/// `block_in_place` cannot.
pub(crate) async fn run_guest<F, T>(operation: F) -> anyhow::Result<T>
where
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|e| anyhow::anyhow!("plugin call task failed: {e}"))?
}
