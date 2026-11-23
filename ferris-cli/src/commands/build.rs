//! Build command: compile the app crate to wasm32-wasip2 component
//! and deposit it in `plugins/<name>.wasm`.

use super::*;
use crate::config::CliConfig;
use crate::project;
use ferris_wasm::load_component;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

pub fn run(args: super::BuildArgs, config: &CliConfig) -> Result<()> {
    let app_dir = resolve_app_dir(config, args.app.as_deref())?;
    project::check_project_exists(&app_dir)?;

    let package_name = build_plugin(&app_dir, &args.profile)?;
    write_package(&app_dir, &package_name)?;
    info!(
        "Build completed successfully! Catalogue package: {}/{package_name}.wasm",
        app_dir.display()
    );

    if let Some(out_dir) = args.out_dir.clone() {
        copy_artifacts(&app_dir, &out_dir, &package_name, &args.profile)?;
    }
    Ok(())
}

/// Locate the app the command operates on: `--app`, the current app directory,
/// or the only app under `<bench>/apps`.
pub(crate) fn resolve_app_dir(config: &CliConfig, requested: Option<&str>) -> Result<PathBuf> {
    if let Some(name) = requested {
        let dir = config.source.bench_root.join("apps").join(name);
        if !dir.join("Cargo.toml").is_file() {
            anyhow::bail!(
                "app `{name}` not found at {}; run `ferris new {name}` first",
                dir.display()
            );
        }
        return Ok(dir);
    }

    let project_dir = config.source.project_dir.clone();
    if project_dir.join("Cargo.toml").is_file() {
        return Ok(project_dir);
    }

    let apps_dir = config.source.bench_root.join("apps");
    let mut apps: Vec<PathBuf> = std::fs::read_dir(&apps_dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.join("Cargo.toml").is_file())
                .collect()
        })
        .unwrap_or_default();
    apps.sort();
    match apps.len() {
        1 => Ok(apps.remove(0)),
        0 => anyhow::bail!(
            "no app found under {}; run `ferris new <name>` or `ferris build --app <name>`",
            apps_dir.display()
        ),
        _ => {
            let names: Vec<String> = apps
                .iter()
                .filter_map(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .collect();
            anyhow::bail!(
                "several apps under {} ({}); run `ferris build --app <name>` or cd into apps/<name>",
                apps_dir.display(),
                names.join(", ")
            )
        }
    }
}

/// Write the catalogue package next to the source.
///
/// `ferris site install-app` reads exactly this shape (`plugin.toml`,
/// `<app>.wasm`, `world/`), so a build produces something installable. Files
/// that already exist are preserved: the manifest carries the app's version.
fn write_package(app_dir: &Path, package_name: &str) -> Result<()> {
    let component = plugin_path(app_dir, package_name);
    let dest = app_dir.join(format!("{package_name}.wasm"));
    if component != dest {
        std::fs::copy(&component, &dest)?;
    }

    let manifest = app_dir.join(ferris_core::site::PLUGIN_MANIFEST);
    if !manifest.is_file() {
        std::fs::write(
            &manifest,
            format!("name = \"{package_name}\"\nversion = \"0.1.0\"\n"),
        )?;
    }

    let world = app_dir.join(ferris_core::site::TEMPLATE_WORLD);
    for dir in ferris_core::plugins::WORLD_DIRS {
        std::fs::create_dir_all(world.join(dir))?;
    }
    // Exported customizations travel in the template; dev data/files/secrets
    // do not (the template creates them empty).
    let exported = app_dir.join("customizations");
    if exported.is_dir() {
        for entry in std::fs::read_dir(&exported)?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                std::fs::copy(&path, world.join("customizations").join(entry.file_name()))?;
            }
        }
    }
    std::fs::create_dir_all(app_dir.join("assets"))?;
    Ok(())
}

/// The package name from the project's `Cargo.toml`.
pub(crate) fn package_name(project_dir: &Path) -> Result<String> {
    let manifest = project_dir.join("Cargo.toml");
    let content = fs::read_to_string(&manifest)
        .map_err(|e| anyhow::anyhow!("could not read {}: {e}", manifest.display()))?;
    let doc = content.parse::<toml_edit::DocumentMut>()?;
    // `doc["package"]["name"]` panics on a manifest with no `[package]` table;
    // indexing a missing table is not a missing key.
    let name = doc
        .get("package")
        .and_then(|item| item.as_table_like())
        .and_then(|package| package.get("name"))
        .and_then(|item| item.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing package.name in Cargo.toml"))?;
    Ok(name.to_string())
}

/// Build the plugin for `wasm32-wasip2` and write it to `plugins/<package>.wasm`.
///
/// Returns the package name. `deploy` calls this instead of a bare
/// `cargo build --release`, which produced a `.wasm` that was never turned into
/// a component and therefore never deployed.
pub(super) fn build_plugin(project_dir: &Path, profile: &str) -> Result<String> {
    let package_name = package_name(project_dir)?;
    info!("Building {} for wasm32-wasip2 ({profile})", package_name);

    // The app's .cargo/config.toml already sets target = "wasm32-wasip2"
    // so a plain `cargo build` will target wasip2.
    let status = std::process::Command::new("cargo")
        .current_dir(project_dir)
        .args(["build", "--profile", profile])
        .status()?;
    if !status.success() {
        return Err(anyhow::anyhow!("WASM build failed"));
    }

    write_component(project_dir, &package_name, profile)?;
    Ok(package_name)
}

fn write_component(project_dir: &Path, package_name: &str, profile: &str) -> Result<()> {
    // Where cargo actually put it. The `dev` profile lands in `debug/` and the
    // file is named after the crate, so a package called `my-app` is `my_app.wasm`;
    // looking for `target/wasm32-wasip2/dev/my-app.wasm` found nothing, and the
    // error said "Did cargo build succeed?" about a build that had.
    let core_wasm = super::dev::app_core_wasm_path(project_dir, package_name, profile);

    if !core_wasm.exists() {
        return Err(anyhow::anyhow!(
            "Core wasm not found at {}. Did cargo build succeed?",
            core_wasm.display()
        ));
    }

    // Use ferris-wasm pipeline to ensure it's a component
    let component = load_component(&core_wasm)?;

    // Write to plugins/<name>.wasm. The plugin is named after the package, so this
    // one keeps the dash.
    let out_path = plugin_path(project_dir, package_name);
    fs::create_dir_all(out_path.parent().expect("a file has a parent"))?;
    fs::write(&out_path, component)?;

    info!("Component written to {}", out_path.display());
    Ok(())
}

/// Where the built plugin lives: named after the package, unlike the artifact
/// cargo produced.
pub(super) fn plugin_path(project_dir: &Path, package_name: &str) -> PathBuf {
    project_dir
        .join("plugins")
        .join(format!("{package_name}.wasm"))
}

fn copy_artifacts(
    project_dir: &Path,
    out_dir: &str,
    package_name: &str,
    profile: &str,
) -> Result<()> {
    let out_path = Path::new(out_dir);
    fs::create_dir_all(out_path)?;

    // An app crate is a `cdylib` for `wasm32-wasip2`, so cargo produces no native
    // binary to copy. The old code asked for `CARGO_PKG_NAME` — a compile-time
    // variable read at runtime, therefore always absent, therefore always the
    // fallback name — looked for a file that never exists, and logged "Copied
    // binary" only if it did.
    let crate_name = package_name.replace('-', "_");
    let out_dir_name = if profile == "dev" { "debug" } else { profile };
    let native = project_dir
        .join("target")
        .join(out_dir_name)
        .join(&crate_name);
    if native.is_file() {
        fs::copy(&native, out_path.join(&crate_name))?;
        info!(
            "Copied native binary to {}",
            out_path.join(&crate_name).display()
        );
    } else {
        info!(
            "No native binary for {crate_name} (a WASM app does not produce one); \
             copying the plugin only."
        );
    }

    // Copy WASM component
    let wasm_dst = out_path.join(format!("{package_name}.wasm"));
    fs::copy(plugin_path(project_dir, package_name), &wasm_dst)?;
    info!("Copied component to {}", wasm_dst.display());

    // Copy config files
    let config_src = project_dir.join("config");
    if config_src.is_dir() {
        project::copy_dir_all(&config_src, &out_path.join("config"))?;
    }

    // Copy static files
    let static_src = project_dir.join("static");
    if static_src.is_dir() {
        project::copy_dir_all(&static_src, &out_path.join("static"))?;
    }

    // Copy templates
    let templates_src = project_dir.join("templates");
    if templates_src.is_dir() {
        project::copy_dir_all(&templates_src, &out_path.join("templates"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plugin_keeps_the_dash_and_the_artifact_does_not() {
        let dir = Path::new("/srv/app");
        assert_eq!(
            plugin_path(dir, "my-app"),
            PathBuf::from("/srv/app/plugins/my-app.wasm"),
            "the plugin is named after the package"
        );
        assert_eq!(
            super::super::dev::app_core_wasm_path(dir, "my-app", "dev"),
            PathBuf::from("/srv/app/target/wasm32-wasip2/debug/my_app.wasm"),
            "cargo names the file after the crate, and writes dev to debug/"
        );
        assert_eq!(
            super::super::dev::app_core_wasm_path(dir, "my-app", "release"),
            PathBuf::from("/srv/app/target/wasm32-wasip2/release/my_app.wasm")
        );
    }

    #[test]
    fn a_package_name_comes_from_the_manifest() {
        let dir = std::env::temp_dir().join(format!("ferris-build-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        assert_eq!(package_name(&dir).unwrap(), "my-app");

        std::fs::write(dir.join("Cargo.toml"), "[dependencies]\nserde = \"1\"\n").unwrap();
        let err = package_name(&dir).expect_err("no package table");
        assert!(err.to_string().contains("Missing package.name"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
