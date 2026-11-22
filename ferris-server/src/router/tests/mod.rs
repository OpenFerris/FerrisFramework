//! Tests for the fused router, grouped by the concern they pin.
//!
//! Modules follow the feature of the code they exercise, so an `api`-only build
//! has no test reaching for a handler it did not compile.

#[cfg(feature = "web")]
mod assets;
#[cfg(feature = "web")]
mod auth_guard;
mod device_auth;
#[cfg(all(feature = "print", feature = "report"))]
mod modules;
mod not_found;
#[cfg(feature = "web")]
mod pages;
#[cfg(all(feature = "web", feature = "wasm"))]
mod plugin_request;
mod security;
mod url;
