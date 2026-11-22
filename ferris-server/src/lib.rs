//! Ferris unified server.
//!
//! Binds a single address and serves the subsystems selected by the
//! [`ServerRoles`]: REST API, the desk web UI, realtime socket.io, plus the
//! worker/scheduler roles (background handlers run in [`background`]).
//!
//! # Compile-time features
//!
//! Every vertical is a Cargo feature (see `ferris-server/Cargo.toml`): a build
//! without it does not link the crate, mount its routes or start its role.
//! `--no-default-features --features api` is a REST-only server;
//! `default` is every vertical the framework has shipped.
//!
//! Two gates decide what a process actually serves, and they are deliberately
//! different layers:
//!
//! * **compile time** — the feature does not exist in the binary. Modules and
//!   their dependencies are behind `#[cfg(feature = …)]`, so the API-only build
//!   links no Leptos, no wasmtime and no broker client.
//! * **run time** — `[features]` in the config (`ferris_core::FeatureFlags`).
//!   `false` removes a subsystem that *was* compiled in; [`server::gate_roles`]
//!   turns that into "the role does not start".
//!
//! A `[features]` flag that asks for something the binary was not built with is
//! a boot error, never a silent no-op — see
//! [`server::validate_compiled_features`]. [`host_features`] is the single
//! source of truth for "what was compiled", published process-wide for the WIT
//! `platform::features` import and `ferris doctor`.

#![recursion_limit = "512"]

#[cfg(feature = "jobs")]
pub mod background;
pub mod catalog;
pub mod client_identity;
pub mod doctypes;
pub mod host_features;
pub mod observability;
pub mod pki;
#[cfg(feature = "wasm")]
pub mod plugin_net;
#[cfg(feature = "wasm")]
pub mod plugin_process;
pub mod roles;
pub mod router;
pub mod server;
pub mod shutdown;
#[cfg(feature = "wasm")]
pub mod sse;
pub mod tls;
pub mod workspaces;

pub use client_identity::{PluginClientIdentity, current_client_identity, with_client_identity};
pub use ferris_core::host_features::HostFeatures;
pub use roles::{ServerRole, ServerRoles};
pub use router::build_router;
pub use server::{Server, ServerSettings};
pub use tls::{
    CertificateRevocation, ClientIdentity, CombinedRevocation, TlsClientInfo, TlsListener,
    build_server_config,
};
