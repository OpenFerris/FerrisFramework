//! The config files the scaffold ships.
//!
//! The bench layout (`apps/` + `sites/`) has **one** config at its root, so
//! `ferris build`, `ferris dev`, `ferris app` and `ferris site` all read the
//! same file no matter which app directory the command runs in. A
//! `--standalone` app keeps its own `config/ferris.toml`.

use anyhow::Result;
use std::fs;
use std::path::Path;

/// The bench config: SQLite so it boots with no external database, the global
/// app catalogue in `apps/`, and the locales the binary ships.
pub(super) fn render_bench_toml() -> String {
    r#"# Ferris bench configuration.
#
# One file for every app under `apps/` and every site under `sites/`.
[database]
type = "sqlite"
name = "ferris.db"
host = ""
port = 0
user = ""
password = ""
max_connections = 5
ssl_mode = "disable"

[server]
host = "127.0.0.1"
port = 8000
roles = ["api", "web", "realtime"]

[apps]
# The global catalogue: `ferris build` writes `apps/<app>/<app>.wasm` here and
# `ferris site install-app` copies that package into a site.
wasm_dir = "apps"
enabled = []

[i18n]
default_locale = "en"
supported = ["en", "es"]
"#
    .to_string()
}

/// A standalone app's config, for `ferris new --standalone`.
pub(super) fn render_standalone_toml(name: &str) -> String {
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
host = "127.0.0.1"
port = 8000
roles = ["api", "web", "realtime"]

[apps]
wasm_dir = "plugins"
enabled = []

[i18n]
default_locale = "en"
supported = ["en", "es"]
"#
    )
}

/// Write `<root>/config/ferris.toml` when it does not exist.
///
/// Returns `true` when this call created the file, so a failed scaffold can
/// roll back only what it created. An existing config is never overwritten.
pub(super) fn write_bench_config(root: &Path) -> Result<bool> {
    let dir = root.join("config");
    fs::create_dir_all(&dir)?;
    let path = dir.join("ferris.toml");
    if path.exists() {
        return Ok(false);
    }
    fs::write(&path, render_bench_toml())?;
    Ok(true)
}

/// Write a standalone app's own `config/ferris.toml`.
pub(super) fn write_app_config(app_dir: &Path, name: &str) -> Result<()> {
    let dir = app_dir.join("config");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("ferris.toml"), render_standalone_toml(name))?;
    Ok(())
}

/// The package's access rules, applied by `ferris site install-app`.
///
/// A WASM descriptor has no permissions field, so without this file the app's
/// `DocType`s stay `System Manager`-only until an operator writes rows by hand.
/// The scaffold grants the administrator full rights and leaves a commented
/// example for another role.
pub(super) fn write_docperm(app_dir: &Path, _name: &str) -> Result<()> {
    fs::write(
        app_dir.join("docperm.toml"),
        r#"# Access rules applied when a site installs this app.
#
# One [[permissions]] entry per role and DocType, same shape as
# `ferris role-permissions set`. Rights default to false. The special role
# "All" means every visitor, including anonymous ones.
[[permissions]]
doctype = "Task"
role = "System Manager"
read = true
write = true
create = true
delete = true

# Example: let every signed-in user read the app's documents.
# [[permissions]]
# doctype = "Task"
# role = "All"
# read = true
"#,
    )?;
    Ok(())
}
