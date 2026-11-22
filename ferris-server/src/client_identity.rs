//! Ambient (task-scoped) client-certificate identity for a request.
//!
//! When mutual TLS is enabled, the connection's `ConnectInfo<TlsClientInfo>`
//! carries the verified certificate's facts. Two shapes of handler need them:
//!
//! * the plugin REST handlers (`/plugins/*`) receive the raw request, so they
//!   read the identity straight from request extensions; and
//! * the RPC handlers (`/api/method/*`) receive only the decoded parameters, so
//!   they read the ambient task-local scoped here before the blocking WASM call.
//!
//! [`bind_client_identity`] installs both (request extension + task-local), so
//! every forwarder sees the same identity. Plain-HTTP deployments have no
//! `ConnectInfo<TlsClientInfo>` and simply see `None` — the host stays
//! anonymous rather than fabricating an identity.

use axum::extract::{ConnectInfo, Request};
use axum::middleware::Next;
use axum::response::Response;

use crate::tls::TlsClientInfo;

pub use ferris_core::ClientIdentity;

tokio::task_local! {
    /// Client-certificate identity of the request currently executing.
    static CURRENT_CLIENT_IDENTITY: ClientIdentity;
}

/// Request extension carrying the verified identity (`None` when the caller
/// presented no certificate), mirroring the session user's `PluginUser`
/// extension. Read by the plugin REST/stream handlers.
#[derive(Clone)]
pub struct PluginClientIdentity(pub Option<ClientIdentity>);

/// Run `fut` with `identity` as the ambient client identity. `None` scopes
/// nothing — the guest sees no identity, exactly as on plain HTTP.
pub async fn with_client_identity<T, F>(identity: Option<ClientIdentity>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    match identity {
        Some(identity) => CURRENT_CLIENT_IDENTITY.scope(identity, fut).await,
        None => fut.await,
    }
}

/// The identity scoped for the current task, if any.
///
/// Read it **before** the blocking WASM call: `block_in_place` runs the closure
/// outside the task's polling context, where the task-local is not visible
/// (the same constraint `ferris_document::current_user` documents).
pub fn current_client_identity() -> Option<ClientIdentity> {
    CURRENT_CLIENT_IDENTITY.try_with(Clone::clone).ok()
}

/// Extract the request's client-certificate identity from the connection's
/// `ConnectInfo`, stash it in request extensions and scope it as the ambient
/// identity for `next`.
///
/// `into_make_service_with_connect_info::<TlsClientInfo>()` inserts the
/// `ConnectInfo` extension only for TLS connections; every other deployment
/// (and anonymous mTLS clients) yields `None`.
pub async fn bind_client_identity(mut request: Request, next: Next) -> Response {
    let identity = request
        .extensions()
        .get::<ConnectInfo<TlsClientInfo>>()
        .and_then(|ConnectInfo(info)| info.identity.clone());
    request
        .extensions_mut()
        .insert(PluginClientIdentity(identity.clone()));
    // Also expose the bare `ClientIdentity` (a `ferris-core` type) when a
    // certificate was verified, so `ferris-auth`'s layers — which cannot see
    // `PluginClientIdentity` — can treat it as a credential: `require_csrf`
    // exempts it and the API's `require_auth_or_device` accepts it on
    // `/api/method/*` (H1 / plan Fase 2). Absent for anonymous callers.
    if let Some(identity) = &identity {
        request.extensions_mut().insert(identity.clone());
    }
    with_client_identity(identity, next.run(request)).await
}
