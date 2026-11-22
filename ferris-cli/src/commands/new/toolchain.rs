//! The scaffold's toolchain pin, and the early warning when the target is
//! missing.

use anyhow::Result;
use std::fs;
use std::path::Path;
use tracing::warn;

/// The scaffold's toolchain pin: the component target, so rustup installs it
/// before the first build. Without `targets` a fresh machine fails at first
/// `cargo check` with `can't find crate for core` — the exact failure
/// reproduced on 2026-10-07 when the default toolchain only had
/// `wasm32-wasip1`. The framework's own
/// `rust-toolchain.toml` solves the same problem the same way.
pub(super) fn render_toolchain_toml() -> &'static str {
    "# `wasm32-wasip2` is the target `ferris build`/`ferris dev` compile the\n\
     # component with; keep `channel` in sync with the framework's\n\
     # rust-toolchain.toml.\n\
     [toolchain]\n\
     channel = \"1.98.1\"\n\
     components = [\"rust-analyzer\", \"clippy\", \"rustfmt\"]\n\
     targets = [\"wasm32-wasip2\"]\n\
     profile = \"minimal\"\n"
}

pub(super) fn write_toolchain_toml(project_dir: &Path) -> Result<()> {
    fs::write(
        project_dir.join("rust-toolchain.toml"),
        render_toolchain_toml(),
    )?;
    Ok(())
}

/// The generated `.cargo/config.toml` (and `rust-toolchain.toml`) pin
/// `wasm32-wasip2`. Warn early if the toolchain that resolves **in the new
/// project** can't build for it, so the user isn't surprised by a long rustup
/// error later.
pub(super) fn warn_if_target_missing(project_dir: &Path) {
    let Ok(out) = std::process::Command::new("rustup")
        .args(["target", "list", "--installed"])
        .current_dir(project_dir)
        .output()
    else {
        return; // No rustup — can't check; not fatal.
    };
    if !out.status.success() {
        return;
    }
    let installed = String::from_utf8_lossy(&out.stdout);
    if !installed.lines().any(|l| l.trim() == "wasm32-wasip2") {
        warn!(
            "rust target wasm32-wasip2 is not installed; run
rustup target add wasm32-wasip2 before ferris dev"
        );
    }
}
