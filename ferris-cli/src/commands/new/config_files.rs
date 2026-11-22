//! The dev `config/ferris.toml` the scaffold ships.

use anyhow::Result;
use std::fs;
use std::path::Path;

/// Dev configuration: sqlite by default so `ferris dev` boots with no external
/// database. Roles cover the dev server (API + web desk + realtime).
pub(super) fn render_ferris_toml(name: &str) -> String {
    format!(
        r#"# {name} — Ferris configuration (dev)
[database]
type = "sqlite"
name = "{name}.db"
host = ""
port = 0
user = ""
password = ""
max_connections = 5
ssl_mode = "disable"

[server]
host = "0.0.0.0"
port = 8000
roles = ["api", "web", "realtime"]

[apps]
wasm_dir = "plugins"
enabled = []
"#
    )
}

pub(super) fn write_config_files(project_dir: &Path, name: &str) -> Result<()> {
    fs::write(
        project_dir.join("config").join("ferris.toml"),
        render_ferris_toml(name),
    )?;
    Ok(())
}
