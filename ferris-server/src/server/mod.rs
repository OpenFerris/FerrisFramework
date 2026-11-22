//! Server boot: settings, role gating, TLS/PKI setup and the listener lifecycle.

mod boot;
mod gating;
mod run;
mod settings;

#[cfg(test)]
mod feature_tests;
#[cfg(test)]
mod tests;

pub use boot::bootstrap_authz;
#[cfg(feature = "wasm")]
pub use boot::start_plugin_clock;
pub use gating::{gate_roles, validate_compiled_features};
pub use run::Server;
pub use settings::ServerSettings;
