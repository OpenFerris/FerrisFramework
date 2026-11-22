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

/// Clone the project config for `ferris dev`, filling an ephemeral JWT secret
/// when the project has none.
///
/// `serve` and `run` stay fail-closed through [`CliConfig::require_jwt_secret`]:
/// a production process never starts without the operator's secret. The dev
/// server is the one place where the absence is not actionable, because
/// `ferris new` must not invent a credential and a first `ferris dev` should not
/// stop at a configuration error. The generated secret lives only in memory
/// (never written to disk) and changes on every start, so sessions do not
/// survive a restart; setting `FERRIS_JWT_SECRET` restores stable sessions.
pub(crate) fn dev_config(config: &CliConfig) -> CliConfig {
    let mut config = config.clone();
    if config.auth.jwt_secret.is_none() {
        // Two v4 UUIDs give 72 printable characters, comfortably above the
        // 32-character minimum, from the OS CSPRNG.
        let first = uuid::Uuid::new_v4();
        let second = uuid::Uuid::new_v4();
        config.auth.jwt_secret = Some(format!("{first}{second}"));
        tracing::warn!(
            "auth.jwt_secret is not configured; using an ephemeral development secret \
             (sessions end when the dev server restarts). Set FERRIS_JWT_SECRET for stable sessions."
        );
    }
    config
}

#[cfg(test)]
mod tests;
