//! Dev server command: runs api+web+realtime with hot-reload of the
//! app plugin. Must run inside a `ferris new` project directory.

mod artifact;
mod hydrate;
mod project;
mod run;
mod seed;
mod watch;

pub use run::run;

pub(crate) use artifact::artifact_is_current;
pub(crate) use project::{app_core_wasm_path, build_app_wasm, check_ferris_project};
pub(crate) use watch::{start_watcher, watch_and_reload};

use crate::config::CliConfig;

/// Clone the project config for `ferris dev`.
///
/// Sessions now live in `tab_session_store` and are protected by the random
/// `tower-sessions` cookie id, so there is no signing secret to generate: the
/// dev server and `serve` share the same fail-closed auth setup and the dev
/// profile only changes the cookie to the plain (non-`Secure`) name.
pub(crate) fn dev_config(config: &CliConfig) -> CliConfig {
    config.clone()
}

#[cfg(test)]
mod tests;
