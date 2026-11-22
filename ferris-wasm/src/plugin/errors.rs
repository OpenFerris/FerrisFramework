//! Guest errors → the host's typed error taxonomy.

use crate::error::{PluginErrorKind, WasmError};

/// Split a guest-returned WIT `types::error` into its typed kind and payload,
/// so the host can surface the HTTP status the guest chose and the message.
fn wit_error_parts(error: &crate::ferris::framework::types::Error) -> (PluginErrorKind, String) {
    use crate::ferris::framework::types::Error as WitError;
    match error {
        WitError::NotFound(message) => (PluginErrorKind::NotFound, message.clone()),
        WitError::PermissionDenied(message) => (PluginErrorKind::PermissionDenied, message.clone()),
        WitError::ValidationFailed(message) => (PluginErrorKind::ValidationFailed, message.clone()),
        WitError::Conflict(message) => (PluginErrorKind::Conflict, message.clone()),
        WitError::RateLimited(retry_after) => (
            PluginErrorKind::RateLimited(*retry_after),
            "rate limited".to_string(),
        ),
        WitError::Timeout(message) => (PluginErrorKind::Timeout, message.clone()),
        WitError::Internal(message) => (PluginErrorKind::Internal, message.clone()),
    }
}

/// The typed error payload a guest-returned `types::error` carries, as a
/// `PluginError` the host's HTTP layer maps to a status.
pub(super) fn plugin_error(
    name: &str,
    call: &'static str,
    error: &crate::ferris::framework::types::Error,
) -> WasmError {
    let (kind, message) = wit_error_parts(error);
    WasmError::PluginError {
        name: name.to_string(),
        call,
        kind,
        message,
    }
}
