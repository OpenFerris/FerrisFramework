//! Standalone SSR dev server for the desk, driven by `cargo-leptos`.
//!
//! `ferris dev --hot-reload frontend` hands the process over to cargo-leptos,
//! which builds this binary (feature `ssr`) and runs it as the dev server so
//! it can restart it whenever the desk sources change. It mirrors the whole
//! `ferris-server` web+API profile: same shell, hydration scripts, `/pkg`
//! static serving and the REST/auth layer underneath the desk.
//!
//! The database is injected by the supervisor (`ferris-cli`) as
//! `FERRIS_DEV_*` env vars, resolved from the project's own configuration, so
//! the SDK uses the exact same store as the production server. Sessions use the
//! same `tower-sessions` + `axum-login` stack (plain `ferris_session` cookie in
//! dev, `__Host-ferris_session` behind TLS).
//!
//! The binary root only declares the pieces: [`startup`] assembles the router,
//! [`state`] owns the shared extractors, [`auth`] the ambient-user layer and
//! boot helpers, [`assets`] the embedded static files, and [`plugin`] the
//! `ferris-server` plugin twin. `path` in `Cargo.toml` still points at this
//! file, so the `ferris-web-ssr` target is unchanged.
//!
//! `#[path]` is used because a crate root resolves `mod foo;` next to the root
//! file (`src/bin/foo.rs`, which Cargo would also pick up as a new binary
//! target); the modules live in this `ssr/` directory instead.

#![recursion_limit = "512"]

#[path = "ssr/assets.rs"]
mod assets;
#[path = "ssr/auth.rs"]
mod auth;
#[path = "ssr/plugin.rs"]
mod plugin;
#[path = "ssr/startup.rs"]
mod startup;
#[path = "ssr/state.rs"]
mod state;

#[cfg(test)]
#[path = "ssr/tests.rs"]
mod tests;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    startup::run().await
}
