//! Entry point: validate the name, create the tree, roll back on failure.

use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::commands::NewArgs;
use crate::config::CliConfig;

use super::cargo_files::{write_cargo_config, write_cargo_toml};
use super::config_files::{write_app_config, write_bench_config, write_docperm};
use super::git::init_git;
use super::lib_rs::write_lib_rs;
use super::project_docs::{write_bench_gitignore, write_gitignore, write_readme};
use super::structure::{create_project_structure, write_wit};
use super::toolchain::{warn_if_target_missing, write_toolchain_toml};

pub fn run(args: NewArgs, config: &CliConfig) -> Result<()> {
    validate_project_name(&args.name)?;

    let bench_root = config.source.bench_root.clone();
    let app_dir = resolve_app_dir(&args, &bench_root);

    if app_dir.exists() {
        return Err(anyhow::anyhow!(
            "Directory {} already exists",
            app_dir.display()
        ));
    }

    // Locate the WIT *before* creating anything: a missing framework install
    // must not leave a half-built directory behind.
    let wit_source = crate::framework::framework_root()?.join(crate::framework::FRAMEWORK_WIT);
    if !wit_source.is_file() {
        return Err(anyhow::anyhow!(
            "framework WIT not found at {} (set FERRIS_HOME or reinstall ferris-cli)",
            wit_source.display()
        ));
    }

    info!(
        "Creating new Ferris app (WIT/WASM component): {}",
        args.name
    );

    // A bench owns the config and the two fixed directories; a standalone app
    // owns its own config and keeps the historical `plugins/` layout.
    let created_config = if args.standalone {
        false
    } else {
        write_bench_config(&bench_root)?
    };
    if !args.standalone {
        fs::create_dir_all(bench_root.join("apps"))?;
        fs::create_dir_all(bench_root.join("sites"))?;
    }

    if let Err(e) = scaffold(&app_dir, &args.name, &wit_source, args.standalone) {
        if let Err(cleanup_err) = fs::remove_dir_all(&app_dir) {
            warn!(
                "failed to clean up {} after scaffold error: {cleanup_err}",
                app_dir.display()
            );
        }
        if created_config {
            let _ = fs::remove_file(bench_root.join("config").join("ferris.toml"));
        }
        return Err(e);
    }

    // One repository for the bench: a nested `.git` inside `apps/<app>` would
    // hide the app from the bench's history and make `sites/` untrackable.
    if args.standalone {
        init_git(&app_dir)?;
    } else if !bench_root.join(".git").exists() {
        write_bench_gitignore(&bench_root)?;
        init_git(&bench_root)?;
    }

    warn_if_target_missing(&app_dir);

    info!(
        "Project {} created successfully at {}",
        args.name,
        app_dir.display()
    );
    info!("Next steps:");
    if args.standalone {
        info!("  cd {}", app_dir.display());
        info!("  ferris dev");
    } else {
        info!("  cd {}", bench_root.display());
        info!("  ferris dev --app {}", args.name);
        info!("  ferris build --app {}", args.name);
        info!("  ferris site new <site> --host <site>.localhost --db-type sqlite");
    }
    Ok(())
}

/// Where the app source goes: `<bench>/apps/<name>` unless the caller asked for
/// a standalone app (or an explicit directory).
fn resolve_app_dir(args: &NewArgs, bench_root: &Path) -> PathBuf {
    if let Some(into) = args.into.as_deref() {
        return PathBuf::from(into);
    }
    if args.standalone {
        return PathBuf::from(&args.name);
    }
    bench_root.join("apps").join(&args.name)
}

fn scaffold(project_dir: &Path, name: &str, wit_source: &Path, standalone: bool) -> Result<()> {
    create_project_structure(project_dir, standalone)?;
    write_cargo_toml(project_dir, name)?;
    write_cargo_config(project_dir)?;
    write_toolchain_toml(project_dir)?;
    write_wit(project_dir, wit_source)?;
    write_lib_rs(project_dir, name)?;
    if standalone {
        write_app_config(project_dir, name)?;
    }
    write_gitignore(project_dir)?;
    write_readme(project_dir, name)?;
    write_docperm(project_dir, name)?;
    Ok(())
}

/// Project names must be safe as directory names, Cargo package names, and
/// interpolations into TOML/Rust templates. Reject anything that isn't a
/// plain identifier.
pub(super) fn validate_project_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(anyhow::anyhow!("project name cannot be empty"));
    };
    if !first.is_ascii_alphabetic() {
        return Err(anyhow::anyhow!(
            "project name must start with an ASCII letter (got {first:?})"
        ));
    }
    if let Some(bad) = chars.find(|c| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_')) {
        return Err(anyhow::anyhow!(
            "project name may only contain ASCII letters, digits, '-' and '_' \
             (found {bad:?})"
        ));
    }
    Ok(())
}
