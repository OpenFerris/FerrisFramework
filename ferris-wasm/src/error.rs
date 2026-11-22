use thiserror::Error;

/// The classified kind of a guest-returned plugin error, mirroring the WIT
/// `types::error` variant. The HTTP-facing host maps each kind to a status
/// without guessing from free text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginErrorKind {
    /// 404 — the resource does not exist.
    NotFound,
    /// 403 — the caller may not perform the action.
    PermissionDenied,
    /// 422 — the request is rejected on validation.
    ValidationFailed,
    /// 409 — the operation conflicts with the current state.
    Conflict,
    /// 429 — rate limited; payload is the retry-after delay in seconds.
    RateLimited(u32),
    /// 504 — the operation did not complete in time.
    Timeout,
    /// 500 — the plugin failed internally.
    Internal,
}

impl PluginErrorKind {
    /// The HTTP status code the host maps this kind to.
    #[must_use]
    pub const fn status(&self) -> u16 {
        match self {
            Self::NotFound => 404,
            Self::PermissionDenied => 403,
            Self::ValidationFailed => 422,
            Self::Conflict => 409,
            Self::RateLimited(_) => 429,
            Self::Timeout => 504,
            Self::Internal => 500,
        }
    }
}

/// Errors surfaced by the WASM runtime.
#[derive(Debug, Error)]
pub enum WasmError {
    #[error("wasmtime error: {0}")]
    Wasmtime(#[from] wasmtime::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("wasmtime compile cache unavailable: {0}")]
    CompileCache(#[source] anyhow::Error),

    #[error("plugin `{0}` was not found in the plugins directory")]
    PluginNotFound(String),

    #[error("plugin `{name}` failed to instantiate: {source}")]
    Instantiate {
        name: String,
        #[source]
        source: wasmtime::Error,
    },

    #[error("plugin `{name}` trap while calling `{call}`: {source}")]
    Trap {
        name: String,
        call: &'static str,
        #[source]
        source: wasmtime::Error,
    },

    #[error("plugin `{name}` rejected `{call}`: {kind:?}: {message}")]
    PluginError {
        name: String,
        call: &'static str,
        /// The typed kind the guest returned (`types::error`), mapped to an
        /// HTTP status by [`PluginErrorKind::status`].
        kind: PluginErrorKind,
        message: String,
    },
}

impl WasmError {
    /// The HTTP status backing this error, when one is known: plugin rejects
    /// map to their typed kind, traps to a server error. `None` for host-side
    /// failures the caller should keep treating as "the plugin could not serve
    /// this request" (e.g. no plugin loaded / name mismatch).
    #[must_use]
    pub const fn status(&self) -> Option<u16> {
        match self {
            Self::PluginError { kind, .. } => Some(kind.status()),
            Self::Trap { .. } => Some(500),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, WasmError>;
