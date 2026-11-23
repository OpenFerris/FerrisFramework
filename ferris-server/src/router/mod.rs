//! Builds the fused axum router that serves the requested roles on one address.
//!
//! Submodules follow the features that own them: the desk asset/SSR pieces are
//! `web`, the plugin host (`/plugins`, declared routes, streams) is `wasm`, the
//! socket.io shutdown hook is `realtime`. `auth_layers`, `cors`, `fallback`,
//! `modules`, `notifier`, `oauth`, `security` and `state` are part of the API
//! base and always compile.

#[cfg(feature = "web")]
mod assets;
mod auth_layers;
mod build;
mod cors;
mod fallback;
mod i18n;
#[cfg(feature = "wasm")]
mod method_bridge;
pub(crate) mod modules;
mod notifier;
mod oauth;
#[cfg(feature = "web")]
mod pages;
#[cfg(feature = "wasm")]
mod plugin_app;
#[cfg(feature = "wasm")]
pub(crate) mod plugin_http;
#[cfg(feature = "wasm")]
pub mod plugin_routes;
#[cfg(feature = "wasm")]
mod plugin_stream;
#[cfg(feature = "wasm")]
mod plugins;
#[cfg(feature = "realtime")]
mod realtime;
mod security;
mod state;
#[cfg(feature = "web")]
mod web_auth;

#[cfg(test)]
mod tests;

pub use build::build_router;
#[cfg(feature = "wasm")]
pub use plugin_routes::{DeclaredRouteState, RouteMode, declared_route_router};
pub use state::AppState;
#[cfg(feature = "web")]
pub use state::{LeptosConfig, LeptosState};
