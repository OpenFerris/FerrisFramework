//! Boot seams: authz policies, secret keyring, the WASM epoch clock and signed
//! downloads. Each is a narrow step `Server::run` performs before serving.

#[cfg(feature = "wasm")]
use std::sync::Arc;

use ferris_core::Config;
#[cfg(feature = "wasm")]
use ferris_wasm::PluginStore;
use tracing::{info, warn};

/// Install the permission engine from the schema registry, before anything can
/// serve a request.
///
/// Order matters and is the reason this is not inside `build_router`: only
/// DocTypes *already registered* are translated, and the framework ones are
/// registered eagerly by `ferris_api::core_doctypes::register_core_doctypes`,
/// so registration has to happen first. `build_router` calls the install too —
/// it is idempotent — but by then the engine would already exist with the core
/// DocTypes missing from it.
///
/// Extracted from `run` so the boot sequence is tested without a database. The
/// pool is what lets the persisted Role Permissions rows join the same rebuild.
pub async fn bootstrap_authz(
    pool: &ferris_db::Db,
    policy_path: Option<&std::path::Path>,
    modules: &ferris_api::ModuleRegistry,
) -> ferris_core::Result<ferris_api::authz::PolicyInstall> {
    crate::catalog::register_doctype_catalog(modules);
    ferris_api::authz::install_policies_with_role_permissions(pool, policy_path).await
}

/// Install the key material for `Secret` fields.
///
/// A missing key is **not** fatal at boot: an installation that never uses an
/// OAuth/webhook/SMTP credential must keep serving. But if the catalogue
/// declares any `Secret` field, the first write or reveal will fail with
/// `KeyMissing`, which is much easier to diagnose here — so it is said once,
/// loudly, before anything starts. A *malformed* key is fatal: silently
/// continuing would store values no future read could match.
pub(super) fn install_secret_keyring(config: &Config) -> anyhow::Result<()> {
    match ferris_core::secret::install_keyring_from_config(&config.secrets) {
        Ok(true) => info!("secret fields: AEAD keyring installed"),
        Ok(false) => {
            let declared = ferris_schema::list_doctypes_metas()
                .iter()
                .any(|meta| meta.fields.iter().any(|field| field.fieldtype == "Secret"));
            if declared {
                warn!(
                    "secret fields are declared but no AEAD key is configured: writing or \
                     revealing one will fail. Set FERRIS_SECRETS__KEY (or [secrets] key) to a \
                     32-byte hex/base64 value"
                );
            }
        }
        Err(error) => anyhow::bail!("invalid secret-field key material: {error}"),
    }
    Ok(())
}

/// §C4 boot seam: start the WASM epoch clock for whatever plugin store the
/// operator configured.
///
/// The plugin host arms an epoch deadline around every plugin call, but the
/// deadline only fires while a background clock keeps bumping the shared
/// engine's epoch (`SharedRuntime::spawn_epoch_clock`). Before C4 no
/// production path started that clock, so a plugin in an infinite loop hung
/// its worker forever while looking protected. [`Server::run`] calls this at
/// boot for every configured store; keeping the returned handle for the serve
/// lifetime and aborting on shutdown is up to the caller. Returns `None` when
/// no plugins are configured or the store's runtime has epoch interrupts off.
///
/// Compiled only with the `wasm` feature: without a guest runtime there is no
/// deadline to arm.
#[cfg(feature = "wasm")]
pub fn start_plugin_clock(
    plugins: Option<&Arc<PluginStore>>,
) -> Option<tokio::task::JoinHandle<()>> {
    match plugins {
        Some(plugins) => plugins.runtime().spawn_epoch_clock(),
        None => None,
    }
}
