//! Entry point: validate the name, create the tree, roll back on failure.

use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

use crate::commands::NewArgs;

use super::cargo_files::{write_cargo_config, write_cargo_toml};
use super::config_files::write_config_files;
use super::git::init_git;
use super::lib_rs::write_lib_rs;
use super::project_docs::{write_gitignore, write_readme};
use super::structure::{create_project_structure, write_wit};
use super::toolchain::{warn_if_target_missing, write_toolchain_toml};

pub fn run(args: NewArgs) -> Result<()> {
    validate_project_name(&args.name)?;

    let project_dir = args
        .into
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&args.name));

    if project_dir.exists() {
        return Err(anyhow::anyhow!(
            "Directory {} already exists",
            project_dir.display()
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

    // Roll back the directory if any step fails, so `ferris new` is atomic
    // from the user's point of view.
    if let Err(e) = scaffold(&project_dir, &args.name, &wit_source) {
        if let Err(cleanup_err) = fs::remove_dir_all(&project_dir) {
            warn!(
                "failed to clean up {} after scaffold error: {cleanup_err}",
                project_dir.display()
            );
        }
        return Err(e);
    }

    // After the scaffold exists, and resolved *inside it*: rustup picks the
    // active toolchain per directory, so checking from the caller's cwd (as
    // this used to do) reads the caller's toolchain — e.g. the framework
    // repo's pin — and misses the one the new project will actually use.
    warn_if_target_missing(&project_dir);

    info!(
        "Project {} created successfully at {}",
        args.name,
        project_dir.display()
    );
    info!("Next steps:");
    info!("  cd {}", project_dir.display());
    info!("  ferris dev");
    Ok(())
}

fn scaffold(project_dir: &Path, name: &str, wit_source: &Path) -> Result<()> {
    create_project_structure(project_dir)?;
    write_cargo_toml(project_dir, name)?;
    write_cargo_config(project_dir)?;
    write_toolchain_toml(project_dir)?;
    write_wit(project_dir, wit_source)?;
    write_lib_rs(project_dir, name)?;
    write_config_files(project_dir, name)?;
    write_gitignore(project_dir)?;
    write_readme(project_dir, name)?;
    init_git(project_dir)?;
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
