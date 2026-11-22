//! The dev server's shared state: the desk SSR options plus the same
//! API/auth/session/plugin stores `ferris-server` builds.

use std::path::PathBuf;
use std::sync::Arc;

use ferris_api::{ApiState, SessionHandlers};
use ferris_auth::AuthState;
use ferris_wasm::PluginStore;
use leptos::config::LeptosOptions;

/// Hard cap on the body we forward to a WASM plugin (kept below `body_limit_mb`:
/// plugin handlers run inside `block_in_place`, parking a tokio worker).
pub(super) const PLUGIN_BODY_LIMIT_BYTES: usize = 1024 * 1024;

/// Unified state for the dev router: desk SSR options + the API/auth/session
/// stores that `ferris-server` builds for its production profile.
#[derive(Clone)]
pub(super) struct DevState {
    pub leptos: LeptosOptions,
    pub api: ApiState,
    pub auth: AuthState,
    pub session: SessionHandlers,
    pub plugins: Arc<PluginStore>,
    pub reload: ReloadState,
}

/// Loopback-only plugin override: hot-swaps the app wasm into the store.
#[derive(Clone)]
pub(super) struct ReloadState {
    pub app: Option<AppWasm>,
    pub token: String,
}

/// The project app plugin, as forwarded by the `ferris dev` supervisor.
#[derive(Clone)]
pub(super) struct AppWasm {
    pub name: String,
    pub core_wasm: PathBuf,
}

impl axum::extract::FromRef<DevState> for LeptosOptions {
    fn from_ref(state: &DevState) -> LeptosOptions {
        state.leptos.clone()
    }
}

impl axum::extract::FromRef<DevState> for ApiState {
    fn from_ref(state: &DevState) -> ApiState {
        state.api.clone()
    }
}

// NOTE: allocates a fresh `Arc` per extraction, mirroring `ferris-server`.
impl axum::extract::FromRef<DevState> for Arc<ApiState> {
    fn from_ref(state: &DevState) -> Self {
        Arc::new(state.api.clone())
    }
}

impl axum::extract::FromRef<DevState> for AuthState {
    fn from_ref(state: &DevState) -> AuthState {
        state.auth.clone()
    }
}

impl axum::extract::FromRef<DevState> for SessionHandlers {
    fn from_ref(state: &DevState) -> SessionHandlers {
        state.session.clone()
    }
}

impl axum::extract::FromRef<DevState> for Arc<PluginStore> {
    fn from_ref(state: &DevState) -> Self {
        state.plugins.clone()
    }
}

impl axum::extract::FromRef<DevState> for ReloadState {
    fn from_ref(state: &DevState) -> ReloadState {
        state.reload.clone()
    }
}
