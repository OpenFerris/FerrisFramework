//! Directories and the WIT contract copy.

use anyhow::Result;
use std::fs;
use std::path::Path;

pub(super) fn create_project_structure(project_dir: &Path, with_config: bool) -> Result<()> {
    for dir in ["src", "wit", ".cargo"] {
        fs::create_dir_all(project_dir.join(dir))?;
    }
    // The bench owns `config/ferris.toml`; only a standalone app carries its
    // own, so the directory is not created empty in the bench layout.
    if with_config {
        fs::create_dir_all(project_dir.join("config"))?;
    }
    Ok(())
}

pub(super) fn write_wit(project_dir: &Path, wit_source: &Path) -> Result<()> {
    let wit = fs::read_to_string(wit_source)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", wit_source.display()))?;
    fs::write(project_dir.join("wit").join("ferris.wit"), wit)?;
    Ok(())
}
