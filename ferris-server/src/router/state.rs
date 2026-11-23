//! Axum application state and the Leptos SSR configuration it carries.
//!
//! The state is assembled from the substates the compiled features provide:
//! Leptos only with `web`, the pubsub bus and the plugin store only with `wasm`,
//! `api`, `auth` and `session` are
//! always there — they are what an API-only server is made of.

use std::sync::Arc;

use axum::extract::FromRef;
use ferris_api::{ApiState, OAuthState, SessionHandlers};
use ferris_auth::AuthState;
use ferris_core::I18n;
#[cfg(feature = "web")]
use leptos_config::{Env, LeptosOptions};

#[cfg(feature = "wasm")]
use crate::sse::PubsubBus;

/// Minimal cloneable Leptos configuration for hydration.
/// Mirrors the necessary fields from `LeptosOptions` which is not `Clone`.
#[cfg(feature = "web")]
#[derive(Clone, Debug)]
pub struct LeptosConfig {
    pub output_name: Arc<str>,
    pub site_root: Arc<str>,
    pub site_pkg_dir: Arc<str>,
    pub env: Env,
    /// Dev mode: serve `/static/desk.css` from this file so theme edits apply
    /// without a server restart. `None` → serve the embedded stylesheet.
    pub desk_css_dev: Option<Arc<std::path::Path>>,
}

#[cfg(feature = "web")]
impl From<LeptosConfig> for LeptosOptions {
    fn from(cfg: LeptosConfig) -> Self {
        Self::builder()
            .output_name(cfg.output_name)
            .site_root(cfg.site_root)
            .site_pkg_dir(cfg.site_pkg_dir)
            .env(cfg.env)
            .build()
    }
}

#[cfg(feature = "web")]
impl FromRef<LeptosConfig> for LeptosOptions {
    fn from_ref(cfg: &LeptosConfig) -> Self {
        cfg.clone().into()
    }
}

/// Shared state for Leptos SSR.
#[cfg(feature = "web")]
#[derive(Clone)]
pub struct LeptosState {
    pub(super) config: LeptosConfig,
}

#[cfg(feature = "web")]
impl FromRef<LeptosState> for LeptosOptions {
    fn from_ref(state: &LeptosState) -> Self {
        state.config.clone().into()
    }
}

/// Unified application state containing all substates.
#[derive(Clone)]
pub struct AppState {
    /// SSR configuration: desk asset paths, Leptos env. Absent without `web`.
    #[cfg(feature = "web")]
    pub leptos: LeptosState,
    pub api: ApiState,
    pub auth: AuthState,
    /// Public session endpoints (login/logout/session): shares the auth
    /// middleware's session store and the api pool.
    pub session: SessionHandlers,
    /// The OAuth2 login surface (`/api/oauth/*`). Built here, alongside the
    /// session handlers, because the callback opens a session on the *same*
    /// `SessionManager`; empty (no providers) when the operator configured none.
    pub oauth: OAuthState,
    /// Process-wide pubsub bus backing the `/events` SSE bridge. Present only
    /// with `wasm`: the bus is the plugin `pubsub::publish` fan-out, so a build
    /// with no guest runtime has nothing to publish.
    #[cfg(feature = "wasm")]
    pub pubsub: PubsubBus,
    /// Public origin the Desk is served from (`[server] external_url`), used to
    /// tell the Desk's own API traffic from external API calls when deciding
    /// whether an `API Request Log` row is worth keeping. `None` falls back to
    /// the request's `Host` header; see `observability::is_external_api_request`.
    pub api_log_base_origin: Option<String>,
    /// The loaded app plugins. `None` when this process publishes no apps.
    /// Carried on the state so a declared route (`/plugins/*` aside) can resolve
    /// its app without a second router-wide state type. Absent without `wasm`.
    #[cfg(feature = "wasm")]
    pub plugins: Option<Arc<ferris_wasm::PluginStore>>,
    /// Locale policy and embedded catalogues.
    pub i18n: I18n,
}

#[cfg(feature = "web")]
impl FromRef<AppState> for LeptosOptions {
    fn from_ref(state: &AppState) -> LeptosOptions {
        state.leptos.config.clone().into()
    }
}

impl FromRef<AppState> for ApiState {
    fn from_ref(state: &AppState) -> ApiState {
        state.api.clone()
    }
}

impl FromRef<AppState> for AuthState {
    fn from_ref(state: &AppState) -> AuthState {
        state.auth.clone()
    }
}

impl FromRef<AppState> for SessionHandlers {
    fn from_ref(state: &AppState) -> SessionHandlers {
        state.session.clone()
    }
}

/// The OAuth routes resolve their own state out of `AppState`, which is what
/// lets `Router<AppState>` embed them (`oauth_router_with_state`).
impl FromRef<AppState> for OAuthState {
    fn from_ref(state: &AppState) -> OAuthState {
        state.oauth.clone()
    }
}

#[cfg(feature = "wasm")]
impl FromRef<AppState> for PubsubBus {
    fn from_ref(state: &AppState) -> PubsubBus {
        state.pubsub.clone()
    }
}

impl FromRef<AppState> for I18n {
    fn from_ref(state: &AppState) -> I18n {
        state.i18n.clone()
    }
}

// NOTE: this allocates a fresh `Arc` on every extraction. If any handler relies
// on `Arc<ApiState>` being extracted on the hot path, consider changing
// `AppState::api` to `Arc<ApiState>` and returning `state.api.clone()` here.
impl FromRef<AppState> for Arc<ApiState> {
    fn from_ref(state: &AppState) -> Self {
        Arc::new(ApiState::from_ref(state))
    }
}
