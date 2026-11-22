//! Locating the framework installation.
//!
//! `ferris new` and `ferris generate` both copy `ferris-wasm/wit/ferris.wit` out of
//! the installation, so the search for it belongs to neither of them: living in
//! `new.rs` meant `generate` reached into another command's internals for it.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// The app contract WIT, relative to the framework root.
pub const FRAMEWORK_WIT: &str = "ferris-wasm/wit/ferris.wit";

/// Locate the framework root (the directory that contains `ferris-wasm/`).
///
/// Order of resolution:
/// 1. `FERRIS_HOME` env var (explicit override, works for any layout).
/// 2. Layout next to the running executable — covers `cargo install` and
///    packaged releases where WIT ships under `<prefix>/share/ferris`.
/// 3. `FERRIS_DATA_DIR` pointing at a directory containing `ferris-wasm/wit/ferris.wit`.
/// 4. Embedded WIT in the binary (self-contained).
/// 5. Compile-time workspace layout — only useful when running the CLI
///    straight out of the repo with `cargo run`.
///
/// # Errors
///
/// `FERRIS_HOME` is set and is not a directory, or no candidate layout contains
/// the WIT. The message says so; there is nothing to fall back to.
pub fn framework_root() -> Result<PathBuf> {
    if let Ok(home) = std::env::var("FERRIS_HOME") {
        let path = PathBuf::from(home);
        if path.is_dir() {
            return Ok(path);
        }
        return Err(anyhow::anyhow!(
            "FERRIS_HOME={} is not a directory",
            path.display()
        ));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin_dir) = exe.parent() {
            for candidate in [
                bin_dir.join("../share/ferris"),
                bin_dir.join("../lib/ferris"),
                bin_dir.join(".."), // Windows-ish portable layout
            ] {
                if candidate.join(FRAMEWORK_WIT).is_file() {
                    return Ok(candidate);
                }
            }
        }
    }

    // Embedded WIT (self-contained binary)
    if let Ok(data_dir) = std::env::var("FERRIS_DATA_DIR") {
        let data_dir_path = PathBuf::from(&data_dir);
        let p = data_dir_path.join(FRAMEWORK_WIT);
        if p.is_file() {
            return Ok(data_dir_path);
        }
    }

    // Dev fallback: only meaningful when run from a cargo workspace.
    if let Some(dir) = Path::new(env!("CARGO_MANIFEST_DIR")).parent() {
        if dir.join(FRAMEWORK_WIT).is_file() {
            return Ok(dir.to_path_buf());
        }
    }

    // Embedded WIT (self-contained binary)
    const EMBEDDED_WIT: &str = include_str!("../../ferris-wasm/wit/ferris.wit");
    let temp = std::env::temp_dir().join("ferris-framework");
    let _ = std::fs::create_dir_all(&temp);
    let wit_path = temp.join("ferris-wasm").join("wit");
    let _ = std::fs::create_dir_all(&wit_path);
    let wit_file = wit_path.join("ferris.wit");
    if !wit_file.is_file() {
        let _ = std::fs::write(&wit_file, EMBEDDED_WIT);
    }
    if wit_file.is_file() {
        return Ok(temp);
    }

    Err(anyhow::anyhow!(
        "could not locate the framework WIT. Set FERRIS_HOME to the directory \
         containing `ferris-wasm/wit/ferris.wit`."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repo_layout_is_found() {
        // Running the tests from the workspace, the dev fallback has to work or
        // `ferris new` would not work for anyone developing here.
        let root = framework_root().expect("the framework root in this workspace");
        assert!(
            root.join(FRAMEWORK_WIT).is_file(),
            "{} has no {FRAMEWORK_WIT}",
            root.display()
        );
    }
}
