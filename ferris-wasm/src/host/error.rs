//! Plugin-boundary error taxonomy.
//!
//! Classification lives in one place so a caller can react to a kind
//! ([`HostError`]) instead of matching message text, and the bridge maps the
//! same kind onto the WIT `types::error` variant without a second guess.

/// A framework error classified for the plugin boundary. The concrete kinds
/// mirror the WIT `types::error` variant, and every one maps to an HTTP status
/// without string matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    NotFound(String),
    PermissionDenied(String),
    ValidationFailed(String),
    Conflict(String),
    RateLimited(u32),
    /// A host service this runtime does not implement yet.
    Unimplemented(String),
    /// A request the host understands but cannot honour: a mixed `apply-ops`
    /// batch with creates, a TLS listener without `tls-server`, …
    Unsupported(String),
    Internal(String),
}

impl From<anyhow::Error> for HostError {
    fn from(err: anyhow::Error) -> Self {
        classify_host_error(&err)
    }
}

/// The honest failure of a call the host has not wired yet.
///
/// Some surfaces are implemented by the server (stream
/// listeners, TLS termination, stream writes). The runtime refuses them with
/// this typed error instead of answering `Ok` and pretending: a guest that
/// branches on the failure gets the truth, and the day the server wires the
/// surface the error disappears. It maps to the WIT `internal` variant because
/// `types.error` has no `unimplemented` case — the prefix keeps it greppable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BridgeError {
    /// No implementation exists in this runtime at all.
    #[error("unimplemented: {0}")]
    Unimplemented(String),
    /// The call exists but its shape cannot be honoured (mixed batches,
    /// capabilities that need a profile, …).
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl BridgeError {
    /// The runtime has no implementation of `call`.
    #[must_use]
    pub fn unimplemented(call: impl Into<String>) -> anyhow::Error {
        Self::Unimplemented(call.into()).into()
    }

    /// The runtime cannot honour this shape of `call`.
    #[must_use]
    pub fn unsupported(call: impl Into<String>) -> anyhow::Error {
        Self::Unsupported(call.into()).into()
    }
}

/// Classify an error the host services raised into the plugin-boundary kinds.
///
/// The classification is only as precise as the error type survives the trip:
/// call sites must propagate `ferris_core::Error` (via `?`) instead of
/// string-wrapping it for the exact mapping to do its job.
#[must_use]
pub fn classify_host_error(err: &anyhow::Error) -> HostError {
    if let Some(core) = err.downcast_ref::<ferris_core::Error>() {
        return classify_core_error(core);
    }
    if let Some(bridge) = err.downcast_ref::<BridgeError>() {
        return match bridge {
            BridgeError::Unimplemented(call) => HostError::Unimplemented(call.clone()),
            BridgeError::Unsupported(call) => HostError::Unsupported(call.clone()),
        };
    }
    // A `?` on a `ferris_core::Result` wraps the concrete error as the anyhow
    // source rather than the top level.
    if let Some(source) = err.source() {
        if let Some(core) = source.downcast_ref::<ferris_core::Error>() {
            return classify_core_error(core);
        }
        if let Some(bridge) = source.downcast_ref::<BridgeError>() {
            return match bridge {
                BridgeError::Unimplemented(call) => HostError::Unimplemented(call.clone()),
                BridgeError::Unsupported(call) => HostError::Unsupported(call.clone()),
            };
        }
    }
    if let Some(io) = err.downcast_ref::<std::io::Error>()
        && io.kind() == std::io::ErrorKind::NotFound
    {
        return HostError::NotFound(io.to_string());
    }
    HostError::Internal(err.to_string())
}

fn classify_core_error(core: &ferris_core::Error) -> HostError {
    use ferris_core::Error as Core;
    match core {
        Core::Job(ferris_core::JobError::NotFound(_)) => HostError::NotFound(core.to_string()),
        Core::Validation(_) => HostError::ValidationFailed(core.to_string()),
        Core::Authz(_) => HostError::PermissionDenied(core.to_string()),
        Core::Auth(auth) => match auth {
            ferris_core::AuthError::RateLimited => HostError::RateLimited(0),
            _ => HostError::PermissionDenied(core.to_string()),
        },
        Core::Document(doc) => match doc {
            ferris_core::DocumentError::NotFound { .. } => HostError::NotFound(core.to_string()),
            ferris_core::DocumentError::AlreadyExists { .. } => {
                HostError::Conflict(core.to_string())
            }
            _ => HostError::ValidationFailed(core.to_string()),
        },
        Core::Schema(schema) => match schema {
            ferris_core::SchemaError::DocTypeNotFound(_) => HostError::NotFound(core.to_string()),
            _ => HostError::ValidationFailed(core.to_string()),
        },
        // Everything else the host has no business mapping is internal.
        _ => HostError::Internal(core.to_string()),
    }
}

/// Translate a [`HostApi`] failure into the WIT `types::error` variant the
/// guest receives. Each kind maps to the HTTP status documented on
/// [`HostError`], so the guest can decide how to answer and the host can still
/// translate its decision back to a status code.
pub(crate) fn to_wit_error(err: &anyhow::Error) -> crate::ferris::framework::types::Error {
    use crate::ferris::framework::types::Error as WitError;
    match classify_host_error(err) {
        HostError::NotFound(msg) => WitError::NotFound(msg),
        HostError::PermissionDenied(msg) => WitError::PermissionDenied(msg),
        HostError::ValidationFailed(msg) => WitError::ValidationFailed(msg),
        HostError::Conflict(msg) => WitError::Conflict(msg),
        HostError::RateLimited(retry_after) => WitError::RateLimited(retry_after),
        // `types.error` has no unimplemented/unsupported case; the message keeps
        // the distinction legible for an operator and greppable in a log.
        HostError::Unimplemented(msg) => WitError::Internal(format!("unimplemented: {msg}")),
        HostError::Unsupported(msg) => WitError::Internal(format!("unsupported: {msg}")),
        HostError::Internal(msg) => WitError::Internal(msg),
    }
}
