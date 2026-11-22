//! Resolving `tls-material` against the app's world.
//!
//! A `tls-server`/`tls-client` record can carry a certificate or a private key
//! inline (`pem`) or name a file inside the app's world (`secret`, e.g.
//! `secrets/client.pem`). The file form is the one an app should use: it rotates
//! without a new `describe()`, never appears in a descriptor projection and is
//! owned by the app like the rest of its world. Resolving it needs the world
//! root the plugin store owns, which is why it happens here and not in the
//! guest or in the descriptor parser.
//!
//! A private key is never logged and never included in an error message: the
//! errors name the *key* that failed, not its value.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use ferris_wasm::{PluginStore, TlsMaterial};

/// Resolve one piece of declared TLS material to PEM.
///
/// # Errors
///
/// The app has no world (the `filesystem` capability is not granted), the path is
/// not confined to the world, or the file cannot be read.
pub async fn resolve(
    plugins: &Arc<PluginStore>,
    app: &str,
    material: &TlsMaterial,
    what: &str,
) -> Result<String> {
    match material {
        TlsMaterial::Pem(pem) => Ok(pem.clone()),
        TlsMaterial::Secret(path) => {
            // Serving with material the app does not own would be a capability
            // bypass with extra steps: the file lives in the app's world (or
            // nowhere), and `world_path` refuses anything that escapes it.
            let world = plugins.world_of(app).ok_or_else(|| {
                anyhow::anyhow!(
                    "`{what}` names `{path}`, but app `{app}` has no world (grant the `world` \
                     capability)"
                )
            })?;
            let file = ferris_core::plugins::world_path(&world, Path::new(path.trim()))
                .with_context(|| format!("resolving `{what}` `{path}` for app `{app}`"))?;
            let pem = tokio::fs::read_to_string(&file)
                .await
                .with_context(|| format!("reading `{what}` `{path}` for app `{app}`"))?;
            if pem.trim().is_empty() {
                anyhow::bail!("`{what}` `{path}` is empty for app `{app}`");
            }
            Ok(pem)
        }
    }
}
