//! Project detection and the wasm build/reload cycle.

use anyhow::{Context, Result};
use ferris_wasm::PluginStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;

use crate::commands::plugin_boot::{self, AppComponent};

pub(crate) fn check_ferris_project(dir: &Path) -> Result<()> {
    if !dir.join("Cargo.toml").exists() {
        anyhow::bail!("Not a Ferris project (Cargo.toml not found). Run 'ferris new' first.");
    }
    if !dir.join("wit").join("ferris.wit").exists() {
        anyhow::bail!(
            "Not a Ferris app project (wit/ferris.wit not found). Run 'ferris new' inside the app directory."
        );
    }
    Ok(())
}

pub(crate) fn build_app_wasm(project_dir: &Path, package_name: &str, quiet: bool) -> Result<()> {
    info!("Building app {} (wasm32-wasip2)...", package_name);
    let mut cmd = std::process::Command::new("cargo");
    cmd.current_dir(project_dir)
        .args(["build", "--target", "wasm32-wasip2", "--profile", "dev"]);
    if quiet {
        cmd.arg("--quiet");
    }
    let status = cmd.status()?;
    if !status.success() {
        anyhow::bail!("WASM build failed");
    }
    Ok(())
}

/// Path to the app's compiled core wasm module for `wasm32-wasip2`.
///
/// Cargo writes the `dev` profile to `debug`, and names the file after the crate
/// rather than the package, so `my-app` lands in `my_app.wasm`. Both spellings
/// used to live in two identical functions here, one of which forgot the dash
/// rule: that is how `ferris build` came to look for `my-app.wasm` and fail.
pub(crate) fn app_core_wasm_path(project_dir: &Path, package_name: &str, profile: &str) -> PathBuf {
    let crate_name = package_name.replace('-', "_");
    let out_dir = match profile {
        // `cargo build --profile dev` (and `--profile test`) write to `debug`.
        "dev" | "test" => "debug",
        other => other,
    };
    project_dir
        .join("target")
        .join("wasm32-wasip2")
        .join(out_dir)
        .join(format!("{crate_name}.wasm"))
}

pub(super) fn load_plugin(
    rt: &tokio::runtime::Runtime,
    plugins: &Arc<PluginStore>,
    project_dir: &Path,
    package_name: &str,
    apps: &ferris_core::AppsConfig,
) -> Result<()> {
    let core_wasm = app_core_wasm_path(project_dir, package_name, "dev");
    if !core_wasm.exists() {
        anyhow::bail!(
            "Core wasm not found at {}. Build first.",
            core_wasm.display()
        );
    }

    // The world directory is the app's, in dev too: exports and runtime data do
    // not move to a different place when the app is deployed.
    let dir = plugin_boot::plugins_dir(project_dir, apps);
    let world = ferris_core::plugins::ensure_world_at(&dir, package_name)
        .with_context(|| format!("cannot prepare the world for app `{package_name}`"))?;
    tracing::debug!(app = %package_name, world = %world.display(), "app world ready");

    // Same publication path as `ferris serve` (compile → register DocTypes and
    // lifecycle events → publish), so a component cannot pass in dev and fail
    // in production for a reason that belongs to the boot order.
    plugin_boot::load(
        rt,
        plugins,
        &AppComponent {
            name: package_name.to_string(),
            path: core_wasm,
        },
    )
}

pub(super) fn rebuild_and_reload(
    rt: &tokio::runtime::Runtime,
    plugins: &Arc<PluginStore>,
    project_dir: &Path,
    package_name: &str,
    apps: &ferris_core::AppsConfig,
    verbose: bool,
) -> Result<()> {
    build_app_wasm(project_dir, package_name, !verbose)?;
    load_plugin(rt, plugins, project_dir, package_name, apps)?;
    Ok(())
}
