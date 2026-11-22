//! The guest exports the host calls: describe, requests, events, jobs, streams
//! and UI assets.

use ferris_auth::User;

use super::call::{GuestCallGuard, IN_GUEST_CALL, PER_CALL_EPOCH_BUDGET};
use super::errors::plugin_error;
use super::instance::Plugin;
use crate::error::{PluginErrorKind, Result as FResult, WasmError};
use crate::host::{Capability, GrantSet};
use crate::host_impl::{RealHostApi, RealHostState};
use crate::{HttpRequest, Response, StreamEvent};

/// The route id the host hands to `handle-request` for the app-wide
/// `/plugins/{app}` surface.
///
/// API methods use the reserved `method:<name>` id instead (see
/// [`Plugin::handle_method`]); a declared route passes its own `route.id`.
pub const PLUGIN_ROUTE_ID: &str = "plugins";

/// The largest datagram the runtime forwards into `handle-datagram` (or accepts
/// from it as a reply), in bytes.
///
/// An IP datagram cannot exceed 65 535 bytes, so this is the protocol's own
/// ceiling rather than a policy knob. A transport that hands over more is
/// refused with a typed `validation-failed` **before** the guest runs: a
/// datagram is one atomic message, and truncating it would silently corrupt
/// the protocol the guest implements.
pub const MAX_DATAGRAM_BYTES: usize = 65_535;

impl Plugin {
    /// What the app advertises: `DocTypes`, themes, workspaces, folders and
    /// API methods, as the typed WIT `descriptor` record. Nothing is
    /// parsed from a JSON string by the host anymore.
    ///
    /// A successful description also installs the app's **capability grant**:
    /// the declaration is intersected with the operator profile and becomes
    /// what `platform::grants` reports and what every import checks. Doing it
    /// here means a reload re-computes the grant from the component that is
    /// actually running, rather than from a stale file on disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the guest's `describe` export fails or traps.
    pub fn describe(&self) -> FResult<crate::ferris::framework::descriptor::Descriptor> {
        let described = self.invoke(|store, instance| match instance.call_describe(store) {
            Ok(Ok(describe)) => Ok(describe),
            Ok(Err(error)) => Err(plugin_error(&self.name, "describe", &error)),
            Err(e) => Err(WasmError::Trap {
                name: self.name.clone(),
                call: "describe",
                source: e,
            }),
        })?;
        self.install_grants(&described)?;
        Ok(described)
    }

    /// Drain the `DocType` documents the guest declared through `schema::declare-doctype`
    /// during the last `describe()`.
    ///
    /// Consuming the list is the point: a reload re-runs `describe()`, and a
    /// declaration that stayed staged would be replayed against the registry
    /// on every call. The plugin store validates and registers them before the
    /// component serves a request.
    #[must_use]
    pub fn take_declared_doctypes(&self) -> Vec<String> {
        self.store.lock().data_mut().take_declared_doctypes()
    }

    /// Install the grant the app's declaration earns under the operator
    /// profile, and keep the declared network surface for the router.
    ///
    /// Idempotent: a reload re-installs both from the component that is
    /// actually running.
    ///
    /// # Errors
    ///
    /// An app that declares an `overrides` method without the `overrides`
    /// capability is refused here, before any route or method is published: a
    /// declaration that would silently do nothing is a load failure, like an
    /// unknown capability.
    fn install_grants(
        &self,
        described: &crate::ferris::framework::descriptor::Descriptor,
    ) -> FResult<()> {
        let subscriptions = described.subscriptions.as_ref().map(|channels| {
            channels
                .iter()
                .map(|channel| channel.channel.clone())
                .collect()
        });
        let secret_keys = described
            .config
            .iter()
            .flatten()
            .filter(|item| item.value_type == "secret")
            .map(|item| item.key.clone())
            .collect();
        let mut store = self.store.lock();
        // `threads` has no import to gate: its grant is only meaningful when
        // the engine actually enabled the proposal, so an app on a
        // threads-disabled engine does not get a capability it could never use
        // (the WIT contract says `grants` reports it "when the engine
        // supports it").
        let threads_supported = store.data().threads_supported();
        let declared = described
            .capabilities
            .iter()
            .map(|capability| Capability::from_wit(*capability))
            .filter(|capability| *capability != Capability::Threads || threads_supported);
        store
            .data_mut()
            .set_grants(GrantSet::for_declaration(declared));
        store.data_mut().set_subscriptions(subscriptions);
        store.data_mut().set_secret_config_keys(secret_keys);
        let grants = store.data().grant_set().clone();
        drop(store);

        if !grants.contains(Capability::Overrides)
            && described.methods.iter().any(|method| method.overrides)
        {
            return Err(WasmError::Instantiate {
                name: self.name.clone(),
                source: wasmtime::Error::msg(format!(
                    "app `{}` declares an `overrides` method without the `overrides` capability",
                    self.name
                )),
            });
        }

        *self.network.write() = Some(crate::descriptor::NetworkSpec::from(described));
        Ok(())
    }

    /// Route an HTTP request into the app under the app-wide `/plugins/{app}`
    /// route id (the app-wide entry point, for callers without a route
    /// ids).
    ///
    /// # Errors
    ///
    /// Returns an error when the guest rejects the request or traps.
    pub fn handle_request(&self, req: &HttpRequest) -> FResult<Response> {
        self.handle_request_for_route(PLUGIN_ROUTE_ID, req, None)
    }

    /// Route an HTTP request into the app, bound to `user` as the ambient
    /// identity for the duration of the call, under the app-wide route id.
    ///
    /// # Errors
    ///
    /// Returns an error when the guest rejects the request or traps.
    pub fn handle_request_as(&self, req: &HttpRequest, user: Option<User>) -> FResult<Response> {
        self.handle_request_for_route(PLUGIN_ROUTE_ID, req, user)
    }

    /// Route an HTTP request into the app naming the matched route.
    ///
    /// `route-id` lets one export serve every endpoint without the
    /// guest re-deriving which route the host picked. `route_id` is the
    /// declared `route.id`, [`PLUGIN_ROUTE_ID`] for the app-wide prefix, or
    /// `method:<name>` for a declared API method.
    ///
    /// The ambient identity is set and cleared while the instance lock is
    /// held, so a concurrent request can never observe another caller's
    /// identity. When the request carries a verified `client-identity` and no
    /// user, the boundary binds the device's fail-closed service principal
    /// (`cert:<fingerprint>`, no roles) instead of opting into a global
    /// anonymous bypass: the engine still applies `DocPerm`/`Everyone`. A
    /// certificate with no fingerprint/CN binds nothing and is denied. See
    /// `RealHostApi::certificate_service_user`.
    ///
    /// # Errors
    ///
    /// Returns an error when the guest rejects the request or traps.
    pub fn handle_request_for_route(
        &self,
        route_id: &str,
        req: &HttpRequest,
        user: Option<User>,
    ) -> FResult<Response> {
        self.invoke(|store, instance| {
            // A verified client certificate with no session authenticates a
            // *device*, not a user. Bind the service principal derived from
            // the certificate facts; a real session user always wins over it.
            let certificate_service = user.is_none() && req.client_identity.is_some();
            let user = user.or_else(|| {
                req.client_identity.as_ref().and_then(|identity| {
                    RealHostApi::certificate_service_user(
                        &identity.fingerprint,
                        identity.common_name.as_deref(),
                    )
                })
            });
            store.data().api().set_current_user(user);
            store
                .data()
                .api()
                .set_certificate_service(certificate_service);
            let result = instance.call_handle_request(&mut *store, route_id, req);
            // No stale identity or service mode: a later host call on the same
            // instance starts from anonymous unless its own entry point binds
            // one.
            store.data().api().set_current_user(None);
            store.data().api().set_certificate_service(false);
            match result {
                Ok(Ok(resp)) => Ok(resp),
                Ok(Err(error)) => Err(plugin_error(&self.name, "handle-request", &error)),
                Err(e) => Err(WasmError::Trap {
                    name: self.name.clone(),
                    call: "handle-request",
                    source: e,
                }),
            }
        })
    }

    /// Route a declared API method into `handle-request` under the reserved
    /// `method:<name>` route id, authorized as `user`.
    ///
    /// # Errors
    ///
    /// Returns an error when the guest rejects the request or traps.
    pub fn handle_method(
        &self,
        method: &str,
        req: &HttpRequest,
        user: Option<User>,
    ) -> FResult<Response> {
        self.handle_request_for_route(&format!("method:{method}"), req, user)
    }

    /// Forward a `DocType` lifecycle event to the plugin.
    ///
    /// # Errors
    ///
    /// Returns an error when the guest's `doctype_event` export fails or traps.
    pub fn on_doctype_event(
        &self,
        event: &str,
        doctype: &str,
        name: &str,
        data: &str,
        user: Option<User>,
    ) -> FResult<()> {
        self.invoke(|store, instance| {
            store.data().api().set_current_user(user);
            match instance.call_doctype_event(store, event, doctype, name, data) {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(plugin_error(&self.name, "doctype-event", &error)),
                Err(e) => Err(WasmError::Trap {
                    name: self.name.clone(),
                    call: "doctype-event",
                    source: e,
                }),
            }
        })
    }

    /// Forward a lifecycle event, but only if the instance is idle.
    ///
    /// `Ok(None)` means the instance is mid-call (a `handle_request` on this
    /// thread already holds the store, or another thread is dispatching); a wasm
    /// instance cannot re-enter itself, so the event is dropped instead of
    /// deadlocking on the store lock.
    ///
    /// # Errors
    ///
    /// Returns an error when the guest's `doctype_event` export fails or traps.
    #[allow(clippy::significant_drop_tightening)] // the guard must stay alive while `store` (a borrow of it) is used by the wasm call
    pub fn try_on_doctype_event(
        &self,
        event: &str,
        doctype: &str,
        name: &str,
        data: &str,
        user: Option<User>,
    ) -> FResult<Option<()>> {
        let Some(mut guard) = self.store.try_lock() else {
            return Ok(None);
        };
        let was = IN_GUEST_CALL.with(|flag| flag.replace(true));
        let _call_guard = GuestCallGuard(was);
        guard.set_epoch_deadline(PER_CALL_EPOCH_BUDGET);
        guard.data().api().set_current_user(user);
        let store: &mut wasmtime::Store<RealHostState> = &mut guard;
        match self
            .instance
            .call_doctype_event(store, event, doctype, name, data)
        {
            Ok(Ok(())) => Ok(Some(())),
            Ok(Err(error)) => Err(plugin_error(&self.name, "doctype-event", &error)),
            Err(e) => Err(WasmError::Trap {
                name: self.name.clone(),
                call: "doctype-event",
                source: e,
            }),
        }
    }

    /// Run one background job in the plugin's `handle-job` export, bound to
    /// `user` as the ambient identity for the duration of the call (`None` is
    /// anonymous — the current dev-mode default).
    ///
    /// The app worker claims a queued job and routes it here carrying its
    /// `job-type` and the JSON `payload`; the returned string is the job's
    /// result. This is the entry point that lets a cron schedule re-enter the
    /// guest.
    ///
    /// # Errors
    ///
    /// Returns an error when the guest's `handle-job` export fails or traps.
    pub fn on_job(&self, job_type: &str, payload: &str, user: Option<User>) -> FResult<String> {
        self.invoke(|store, instance| {
            store.data().api().set_current_user(user);
            let result = instance.call_handle_job(&mut *store, job_type, payload);
            // No stale identity: a later host call starts from anonymous unless
            // its own entry point binds a user.
            store.data().api().set_current_user(None);
            match result {
                Ok(Ok(output)) => Ok(output),
                Ok(Err(error)) => Err(plugin_error(&self.name, "handle-job", &error)),
                Err(e) => Err(WasmError::Trap {
                    name: self.name.clone(),
                    call: "handle-job",
                    source: e,
                }),
            }
        })
    }

    /// Forward one chunk of a live stream (`WebSocket`, `gRPC`, TCP…)
    /// to the plugin's `handle-stream` export. **Chunks are data only** in
    /// Open and close arrive through
    /// [`on_stream_event`](Self::on_stream_event), so an empty chunk is a
    /// legitimate frame. Returns the bytes to relay back on the connection (an
    /// empty reply is an ack/drop).
    ///
    /// # Errors
    ///
    /// Returns an error when the guest's `handle-stream` export fails or traps.
    pub fn handle_stream(&self, stream_id: &str, chunk: &[u8]) -> FResult<Vec<u8>> {
        self.handle_stream_as(stream_id, chunk, None)
    }

    /// Forward one frame of a live stream, bound to `user` as the ambient
    /// identity for the duration of the call.
    ///
    /// # Errors
    ///
    /// Returns an error when the guest's `handle-stream` export fails or traps.
    pub fn handle_stream_as(
        &self,
        stream_id: &str,
        chunk: &[u8],
        user: Option<User>,
    ) -> FResult<Vec<u8>> {
        self.invoke(|store, instance| {
            // A stream opened over mutual TLS may carry no session, only the
            // connection's verified certificate. Bind the certificate's
            // fail-closed service principal so the guest's docstore reads run
            // under ordinary policy (DocPerm/Everyone) rather than a global
            // anonymous bypass. A real session user always wins.
            let identity = store.data().api().stream_client_identity(stream_id);
            let certificate_service = user.is_none() && identity.is_some();
            let user = user.or_else(|| {
                identity.as_ref().and_then(|identity| {
                    RealHostApi::certificate_service_user(
                        &identity.fingerprint,
                        identity.common_name.as_deref(),
                    )
                })
            });
            store.data().api().set_current_user(user);
            store
                .data()
                .api()
                .set_certificate_service(certificate_service);
            // Count the frame before the guest sees it, so `streams::info`
            // reports `bytes-received` including the current chunk.
            store.data().api().stream_received(stream_id, chunk.len());
            let result = instance.call_handle_stream(&mut *store, stream_id, chunk);
            store.data().api().set_current_user(None);
            store.data().api().set_certificate_service(false);
            match result {
                Ok(Ok(reply)) => Ok(reply),
                Ok(Err(error)) => Err(plugin_error(&self.name, "handle-stream", &error)),
                Err(e) => Err(WasmError::Trap {
                    name: self.name.clone(),
                    call: "handle-stream",
                    source: e,
                }),
            }
        })
    }

    /// Forward one inbound datagram from a UDP socket, a raw socket or an
    /// ICMP socket to the plugin's `handle-datagram` export, and return the
    /// bytes of one datagram to send back to the same peer (empty drops it).
    ///
    /// `stream-id` is the **socket** the datagram arrived on, not a peer
    /// connection; `peer` is the sender (`"address:port"` for UDP, a bare
    /// address for raw and ICMP). A datagram is one atomic message: the runtime
    /// refuses anything above [`MAX_DATAGRAM_BYTES`] instead of truncating it,
    /// and the same bound applies to the reply.
    ///
    /// # Errors
    ///
    /// Returns an error when the datagram exceeds the bound, the app holds
    /// neither `udp` nor `raw-sockets`, or the guest's `handle-datagram`
    /// export fails or traps.
    pub fn handle_datagram(&self, stream_id: &str, data: &[u8], peer: &str) -> FResult<Vec<u8>> {
        self.handle_datagram_as(stream_id, data, peer, None)
    }

    /// Like [`handle_datagram`](Self::handle_datagram), bound to `user` as the
    /// ambient identity for the duration of the call.
    ///
    /// # Errors
    ///
    /// Same as [`handle_datagram`](Self::handle_datagram).
    pub fn handle_datagram_as(
        &self,
        stream_id: &str,
        data: &[u8],
        peer: &str,
        user: Option<User>,
    ) -> FResult<Vec<u8>> {
        // The socket exists only because the app declared `udp` or
        // `raw-sockets`; re-check at dispatch so a grant replaced by a reload
        // cannot keep feeding a surface the app no longer holds.
        let grants = self.grants();
        if !grants.contains(Capability::Udp) && !grants.contains(Capability::RawSockets) {
            return Err(WasmError::PluginError {
                name: self.name.clone(),
                call: "handle-datagram",
                kind: PluginErrorKind::PermissionDenied,
                message: "handle-datagram needs the `udp` or `raw-sockets` capability".to_string(),
            });
        }
        if data.len() > MAX_DATAGRAM_BYTES {
            return Err(oversized_datagram(&self.name, "received", data.len()));
        }
        let reply = self.invoke(|store, instance| {
            // A datagram socket opened over mutual TLS may carry no session,
            // only the connection's verified certificate; the guest may read
            // the docstore from this export, so it gets the same certificate
            // service principal as `handle_stream_as`.
            let identity = store.data().api().stream_client_identity(stream_id);
            let certificate_service = user.is_none() && identity.is_some();
            let user = user.or_else(|| {
                identity.as_ref().and_then(|identity| {
                    RealHostApi::certificate_service_user(
                        &identity.fingerprint,
                        identity.common_name.as_deref(),
                    )
                })
            });
            store.data().api().set_current_user(user);
            store
                .data()
                .api()
                .set_certificate_service(certificate_service);
            // Count the datagram before the guest sees it, so
            // `streams::info` reports `bytes-received` including this one.
            store.data().api().stream_received(stream_id, data.len());
            let result = instance.call_handle_datagram(&mut *store, stream_id, data, peer);
            store.data().api().set_current_user(None);
            store.data().api().set_certificate_service(false);
            match result {
                Ok(Ok(reply)) => Ok(reply),
                Ok(Err(error)) => Err(plugin_error(&self.name, "handle-datagram", &error)),
                Err(e) => Err(WasmError::Trap {
                    name: self.name.clone(),
                    call: "handle-datagram",
                    source: e,
                }),
            }
        })?;
        if reply.len() > MAX_DATAGRAM_BYTES {
            return Err(oversized_datagram(&self.name, "reply", reply.len()));
        }
        Ok(reply)
    }

    /// Deliver one lifecycle event (`opened`, `closed`, `error`) of a live
    /// stream to the plugin's `handle-stream-event` export.
    ///
    /// Lifecycle is split out of `handle-stream`: per-connection state is
    /// created on `opened` and released on `closed`, and no cleanup is inferred
    /// from a chunk. The event is delivered exactly once per stream and in
    /// order — the caller (the transport) owns that ordering.
    ///
    /// # Errors
    ///
    /// Returns an error when the guest's `handle-stream-event` export fails or
    /// traps.
    pub fn on_stream_event(&self, stream_id: &str, event: StreamEvent) -> FResult<()> {
        self.on_stream_event_as(stream_id, event, None)
    }

    /// Like [`on_stream_event`](Self::on_stream_event), bound to `user` as the
    /// ambient identity for the duration of the call.
    ///
    /// # Errors
    ///
    /// Returns an error when the guest's `handle-stream-event` export fails or
    /// traps.
    #[allow(clippy::needless_pass_by_value)] // the generated guest call takes `&StreamEvent`; the event is the caller's value, not copied here
    pub fn on_stream_event_as(
        &self,
        stream_id: &str,
        event: StreamEvent,
        user: Option<User>,
    ) -> FResult<()> {
        self.invoke(|store, instance| {
            // A lifecycle event may carry per-connection work that reads the
            // docstore (releasing state on `closed`, for instance), so it binds
            // the same certificate service principal as `handle_stream_as`.
            let identity = store.data().api().stream_client_identity(stream_id);
            let certificate_service = user.is_none() && identity.is_some();
            let user = user.or_else(|| {
                identity.as_ref().and_then(|identity| {
                    RealHostApi::certificate_service_user(
                        &identity.fingerprint,
                        identity.common_name.as_deref(),
                    )
                })
            });
            store.data().api().set_current_user(user);
            store
                .data()
                .api()
                .set_certificate_service(certificate_service);
            let result = instance.call_handle_stream_event(&mut *store, stream_id, &event);
            store.data().api().set_current_user(None);
            store.data().api().set_certificate_service(false);
            match result {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(plugin_error(&self.name, "handle-stream-event", &error)),
                Err(e) => Err(WasmError::Trap {
                    name: self.name.clone(),
                    call: "handle-stream-event",
                    source: e,
                }),
            }
        })
    }

    /// Serve one UI asset to the Desk: ask the plugin's `ui-asset` export for
    /// the raw bytes of `path` (script, stylesheet, image, …). The host relays
    /// them at `/plugins/{name}/asset/{path}`.
    ///
    /// # Errors
    ///
    /// Returns an error when the guest's `ui-asset` export fails or traps.
    pub fn ui_asset(&self, path: &str) -> FResult<Vec<u8>> {
        self.invoke(
            |store, instance| match instance.call_ui_asset(store, path) {
                Ok(Ok(bytes)) => Ok(bytes),
                Ok(Err(error)) => Err(plugin_error(&self.name, "ui-asset", &error)),
                Err(e) => Err(WasmError::Trap {
                    name: self.name.clone(),
                    call: "ui-asset",
                    source: e,
                }),
            },
        )
    }

    /// SSR markup for a widget: ask the plugin's `ui-widget` export to render
    /// the definition `widget` with `params` as HTML (the Desk prints it
    /// before the custom element hydrates). Served at
    /// `/plugins/{name}/widget/{widget}?params=…`.
    ///
    /// # Errors
    ///
    /// Returns an error when the guest's `ui-widget` export fails or traps.
    pub fn ui_widget(&self, widget: &str, params: &[(String, String)]) -> FResult<String> {
        self.ui_widget_as(widget, params, None)
    }

    /// Like [`ui_widget`](Self::ui_widget), but bound to `user` as the ambient
    /// identity: the SSR markup is authorized as the real caller, so a widget
    /// that reads documents renders the same rows the user will see live
    /// (`None` is anonymous).
    ///
    /// # Errors
    ///
    /// Returns an error when the guest's `ui-widget` export fails or traps.
    pub fn ui_widget_as(
        &self,
        widget: &str,
        params: &[(String, String)],
        user: Option<User>,
    ) -> FResult<String> {
        self.invoke(|store, instance| {
            store.data().api().set_current_user(user);
            let result = instance.call_ui_widget(&mut *store, widget, params);
            store.data().api().set_current_user(None);
            match result {
                Ok(Ok(html)) => Ok(html),
                Ok(Err(error)) => Err(plugin_error(&self.name, "ui-widget", &error)),
                Err(e) => Err(WasmError::Trap {
                    name: self.name.clone(),
                    call: "ui-widget",
                    source: e,
                }),
            }
        })
    }
}

/// The typed refusal of a datagram above [`MAX_DATAGRAM_BYTES`].
///
/// Truncation is not an option: a datagram is one message, and a guest
/// reassembling a truncated one would answer a protocol frame it never fully
/// received.
fn oversized_datagram(name: &str, direction: &str, len: usize) -> WasmError {
    WasmError::PluginError {
        name: name.to_string(),
        call: "handle-datagram",
        kind: PluginErrorKind::ValidationFailed,
        message: format!(
            "{direction} datagram of {len} bytes exceeds the {MAX_DATAGRAM_BYTES}-byte limit"
        ),
    }
}
