//! WIT import bridges, one file per contract interface.
//!
//! Every imported interface generates its own `Host` trait; [`HostState`]
//! implements all of them and forwards each call into the single
//! [`HostApi`](crate::host::HostApi). One file per interface keeps a service
//! change at one seam:
//!
//! - `data` — documents, blobs, naming series, schema metadata;
//! - `io` — outbound HTTP, streams, pubsub, notifications;
//! - `schema` — the full `DocType` declaration escape hatch;
//! - `platform` — identity, permissions, grants, config, scheduler, pki, features;
//! - `markers` — the data-only `types`/`descriptor` interfaces.
//!
//! Every bridge calls `HostState::require(capability)` before it touches a
//! service, so an ungranted call is the typed `permission-denied` variant
//! rather than a silently restricted one.

use crate::ferris::framework::types::Error as WitError;

mod data;
mod io;
mod markers;
mod platform;
mod schema;

/// Drive a host-service future from a synchronous import callback.
///
/// The component model calls imports synchronously while the guest is running,
/// so the bridge parks on the current tokio runtime; the service errors are
/// classified into the WIT `types.error` variant exactly once, here.
pub(super) fn block_on<T>(
    future: impl std::future::Future<Output = anyhow::Result<T>>,
) -> Result<T, WitError> {
    let runtime = tokio::runtime::Handle::current();
    runtime
        .block_on(future)
        .map_err(|e| crate::host::to_wit_error(&e))
}
