//! Info command
//!
//! Reports what the toolchain and the project look like. It is a diagnostic, so
//! it answers for everything it was asked about rather than stopping at the
//! first thing missing: `ferris info` used to `?` out of `Command::output` and
//! print nothing at all on a machine without `rustc` on the PATH.

use crate::config::CliConfig;
use crate::features;
use anyhow::Result;
use std::path::Path;
use std::process::Command;
use tracing::info;

pub fn run(config: &CliConfig) -> Result<()> {
    print_version_info();
    print_features(config);
    print_tool_info();
    print_project_info(&config.source.project_dir);
    print_dependencies();
    Ok(())
}

/// What this binary contains, next to what `[features]` asks for.
///
/// The compiled set is the answer to "why did `serve` refuse to boot?" without
/// rerunning it, so it is printed even when it agrees with the config. Only
/// switches are shown, never credentials, because `ferris info` output is
/// routinely pasted into issues.
fn print_features(config: &CliConfig) {
    info!("=== Features ===");
    let compiled = features::compiled();
    info!("  compiled: {}", features::describe(&compiled));
    info!(
        "  configured: {}",
        features::configured_summary(&config.features)
    );
    for mismatch in features::mismatches(config, &compiled) {
        info!("  mismatch: {}", mismatch.reason());
    }
}

fn print_version_info() {
    info!("=== Version ===");
    info!(
        "  {}: {}",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    );
}

/// Run `program --version` and report it, or say why it could not be run.
///
/// `doctor` already reports a missing tool instead of propagating the error;
/// `info` did the opposite, so a PATH without rustc produced no output whatsoever
/// instead of a report saying rustc is missing.
fn print_tool_info() {
    info!("=== Toolchain ===");
    for tool in ["rustc", "cargo"] {
        match Command::new(tool).args(["--version"]).output() {
            Ok(output) if output.status.success() => {
                info!(
                    "  {tool}: {}",
                    String::from_utf8_lossy(&output.stdout).trim()
                );
            }
            Ok(output) => info!("  {tool}: `{tool} --version` exited with {}", output.status),
            Err(e) => info!("  {tool}: not available ({e})"),
        }
    }
}

fn print_project_info(project_dir: &Path) {
    info!("=== Project ===");

    let manifest = project_dir.join("Cargo.toml");
    if !manifest.is_file() {
        info!("  not a Cargo project: {}", project_dir.display());
        return;
    }

    // Read the manifest rather than grepping lines: `starts_with("version")`
    // also matched a `[dependencies]`-block version pin, and a commented-out
    // `name = ...` read as the package name.
    let Ok(content) = std::fs::read_to_string(&manifest) else {
        info!("  could not read {}", manifest.display());
        return;
    };
    let Ok(document) = content.parse::<toml_edit::DocumentMut>() else {
        info!("  {} is not valid TOML", manifest.display());
        return;
    };
    for key in ["name", "version", "description"] {
        let value = document["package"][key].as_str();
        match value {
            Some(value) => info!("  {key}: {value}"),
            None if key == "description" => {}
            None => info!("  {key}: (not set)"),
        }
    }
}

fn print_dependencies() {
    info!("=== Dependencies ===");

    match Command::new("cargo")
        .args(["tree", "--depth", "1"])
        .output()
    {
        Ok(output) if output.status.success() => {
            for line in String::from_utf8_lossy(&output.stdout).lines().take(20) {
                info!("  {line}");
            }
        }
        Ok(output) => info!("  `cargo tree` failed ({})", output.status),
        Err(e) => info!("  cargo not available ({e})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_is_read_not_grepped() {
        let dir = std::env::temp_dir().join(format!("ferris-info-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // `version` also appears in a dependency table and in a comment: a line
        // scan reported all three as the package version.
        let manifest = dir.join("Cargo.toml");
        std::fs::write(
            &manifest,
            r#"
[package]
name = "my-app"
# version = "9.9.9"
version = "0.2.0"

[dependencies]
version = "4"
"#,
        )
        .unwrap();

        let document = std::fs::read_to_string(&manifest)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert_eq!(document["package"]["name"].as_str(), Some("my-app"));
        assert_eq!(document["package"]["version"].as_str(), Some("0.2.0"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_directory_without_a_manifest_is_reported_not_fatal() {
        // The signature is `()` on purpose: `ferris info` outside a project is a
        // question with an answer, not a failure.
        print_project_info(Path::new("/definitely/not/a/project"));
    }
}
