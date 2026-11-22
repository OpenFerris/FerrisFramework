//! `ferris app`: the lifecycle of the WASM apps an install publishes.
//!
//! Two scopes, one command (see `auditoria/parches/SITES.md`). An app is
//! installed **globally** once, into `apps/<app>/` (`plugin.toml`, `<app>.wasm`
//! and a `world/` template); a site receives a **copy** of that template plus its
//! own `config.toml` and data through `ferris site install-app`. The wasm is
//! referenced, never duplicated, and upgrading the catalogue never overwrites a
//! site's data.
//!
//! The command keeps the older single-scope layout working: when `[apps] wasm_dir`
//! does not name the global catalogue (`<project>/apps`), the flat
//! `plugins/<app>.wasm` contract of `ferris build` is what the process publishes,
//! and `install`/`remove`/`list` operate on it exactly as before. Deployments that
//! did not opt into `apps/` must not change behaviour because the catalogue
//! exists.
//!
//! `disable` writes an explicit allow-list. An empty list means "every
//! component", so disabling one app has to materialise the names of the others
//! first; that is the only place the legacy path is more than a file move.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use ferris_core::plugins::{WORLD_DIRS, ensure_world_at, is_valid_app_name};
use ferris_core::site::{AppManifest, SiteManager};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use super::AppArgs;
use super::AppCommand;
use super::plugin_boot::{discover, plugins_dir};
use crate::config::CliConfig;

/// Dispatch `ferris app`.
///
/// # Errors
///
/// Whatever the subcommand fails on: an unreadable package, an invalid name, a
/// catalogue app that a site still installs, or an I/O failure under
/// `apps/`/`[apps] wasm_dir`.
pub fn run(args: AppArgs, config: &CliConfig) -> Result<()> {
    let dir = plugins_dir(&config.source.project_dir, &config.apps);
    // The catalogue is the two-scope layout; anything else is an install that
    // still publishes the flat `plugins/` directory.
    let catalogue = SiteManager::default();
    let global = dir == catalogue.apps_root();
    match args.command {
        AppCommand::List { site } => {
            if global {
                list_catalogue(&catalogue, site.as_deref())
            } else {
                list(&dir, &config.apps.enabled)?;
                if let Some(site) = site.as_deref() {
                    println!();
                    list_site(&catalogue, site)?;
                }
                Ok(())
            }
        }
        AppCommand::Install { path, name } => {
            if global {
                install_package(&catalogue, &path, name.as_deref())
            } else {
                install(
                    &dir,
                    &config.source.config_path,
                    &config.apps.enabled,
                    &path,
                    name.as_deref(),
                )
            }
        }
        AppCommand::Remove { name, purge } => {
            if global {
                remove_from_catalogue(&catalogue, &name, purge)
            } else {
                remove(
                    &dir,
                    &config.source.config_path,
                    &config.apps.enabled,
                    &name,
                    purge,
                )
            }
        }
        AppCommand::Enable { name } => set_enabled(
            &dir,
            &config.source.config_path,
            &config.apps.enabled,
            &name,
            true,
        ),
        AppCommand::Disable { name } => set_enabled(
            &dir,
            &config.source.config_path,
            &config.apps.enabled,
            &name,
            false,
        ),
    }
}

// ---------------------------------------------------------------------------
// Global catalogue (`apps/<app>/`)
// ---------------------------------------------------------------------------

/// Install a package directory, or a bare component as a one-app package.
///
/// A directory is validated by [`AppManifest::read`]: it must carry
/// `plugin.toml`, `<app>.wasm` and `world/`. A `.wasm` has no manifest, so a
/// throw-away package is assembled in the temporary directory — manifest, the
/// component and an empty world template — installed from there and deleted.
/// Nothing of that package survives: the catalogue only keeps what it copies.
fn install_package(catalogue: &SiteManager, source: &Path, name: Option<&str>) -> Result<()> {
    let known = catalogue.global_apps()?;
    let manifest = if source.is_dir() {
        catalogue.install_global_app(source)?
    } else {
        install_component(catalogue, source, name)?
    };
    let action = if known.iter().any(|app| app == &manifest.name) {
        "re-installed"
    } else {
        "installed"
    };
    println!(
        "{action} {} {} in the global catalogue at {}",
        manifest.name,
        manifest.version,
        catalogue.global_app_dir(&manifest.name)?.display()
    );
    println!(
        "  add it to a site with `ferris site install-app {} --site <site>`",
        manifest.name
    );
    Ok(())
}

/// Wrap a bare component in a minimal package and install it globally.
fn install_component(
    catalogue: &SiteManager,
    source: &Path,
    name: Option<&str>,
) -> Result<AppManifest> {
    let bytes = fs::read(source)
        .with_context(|| format!("cannot read the component {}", source.display()))?;
    if !ferris_wasm::is_component(&bytes) {
        anyhow::bail!(
            "{} is not a Ferris component (a WebAssembly component of the `ferris-app` world)",
            source.display()
        );
    }
    let name = match name {
        Some(name) => name.to_string(),
        None => source
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow::anyhow!("cannot derive an app name from {}", source.display())
            })?,
    };
    if !is_valid_app_name(&name) {
        anyhow::bail!("invalid app name `{name}`: use letters, digits, `-` or `_` (at most 64)");
    }

    let package = std::env::temp_dir().join(format!("ferris-app-{}", uuid::Uuid::new_v4()));
    let result = build_package(&package, source, &name).and_then(|()| {
        catalogue
            .install_global_app(&package)
            .map_err(anyhow::Error::from)
    });
    // Best effort: a leftover temporary directory must never fail the command,
    // and the install result is what the caller reports.
    let _ = fs::remove_dir_all(&package);
    result
}

/// Assemble the one-app package `install_global_app` expects.
fn build_package(package: &Path, source: &Path, name: &str) -> Result<()> {
    fs::create_dir_all(package).with_context(|| format!("cannot create {}", package.display()))?;
    fs::copy(source, package.join(format!("{name}.wasm")))
        .with_context(|| format!("cannot copy {} into the package", source.display()))?;
    // `version = "0.1.0"` is the component's own version being unknown here:
    // `plugin.toml` is the only place a version is declared, and a bare `.wasm`
    // carries none. The operator overrides it by installing a real package.
    fs::write(
        package.join("plugin.toml"),
        format!("name = \"{name}\"\nversion = \"0.1.0\"\n"),
    )
    .with_context(|| format!("cannot write the manifest in {}", package.display()))?;
    let world = package.join("world");
    for dir in WORLD_DIRS {
        fs::create_dir_all(world.join(dir))
            .with_context(|| format!("cannot create {}", world.display()))?;
    }
    Ok(())
}

fn list_catalogue(catalogue: &SiteManager, site: Option<&str>) -> Result<()> {
    let apps = catalogue.global_apps()?;
    println!("global catalogue: {}", catalogue.apps_root().display());
    if apps.is_empty() {
        println!("  (empty; run `ferris app install <package>`)");
    }
    let sites = catalogue.list()?;
    for app in apps {
        let version = catalogue
            .app_manifest(&app)
            .map_or_else(|_| "?".to_string(), |manifest| manifest.version);
        let installed: Vec<&str> = sites
            .iter()
            .filter(|entry| entry.apps.iter().any(|name| name == &app))
            .map(|entry| entry.name.as_str())
            .collect();
        println!(
            "  {:<24} {:<10} {}",
            app,
            version,
            if installed.is_empty() {
                "not installed in any site".to_string()
            } else {
                format!("sites: {}", installed.join(", "))
            }
        );
    }
    if let Some(site) = site {
        println!();
        list_site(catalogue, site)?;
    }
    Ok(())
}

/// The apps installed in one site, with their instance directory.
fn list_site(catalogue: &SiteManager, site: &str) -> Result<()> {
    let config = catalogue.get(site)?;
    let apps = catalogue.installed_apps(site)?;
    println!("site {} ({})", config.name, config.host_name);
    if apps.is_empty() {
        println!("  (no apps installed)");
        return Ok(());
    }
    for app in apps {
        let instance = catalogue.instance_world(site, &app)?;
        println!("  {:<24} {}", app, instance.display());
    }
    Ok(())
}

/// Remove an app from the catalogue, refusing while a site still installs it.
///
/// Deleting a template that a site's `apps.txt` still names would leave that
/// site unable to reinstall the app and unable to boot with the same config, so
/// the command lists the sites and asks for an explicit uninstall first.
fn remove_from_catalogue(catalogue: &SiteManager, name: &str, purge: bool) -> Result<()> {
    if !is_valid_app_name(name) {
        anyhow::bail!("invalid app name `{name}`");
    }
    let dir = catalogue.global_app_dir(name)?;
    if !dir.is_dir() {
        anyhow::bail!(
            "app `{name}` is not in the global catalogue ({})",
            catalogue.apps_root().display()
        );
    }
    let installed: Vec<String> = catalogue
        .list()?
        .into_iter()
        .filter(|site| site.apps.iter().any(|app| app == name))
        .map(|site| site.name)
        .collect();
    if !installed.is_empty() {
        anyhow::bail!(
            "app `{name}` is still installed in: {}. Run \
             `ferris site uninstall-app {name} --site <site>` for each of them first",
            installed.join(", ")
        );
    }
    if purge {
        // The catalogue holds a template, not site data: there is nothing to
        // purge here, and saying nothing would make the flag look effective.
        println!(
            "note: --purge only affects a site's instance; the global catalogue has no data to delete"
        );
    }
    fs::remove_dir_all(&dir).with_context(|| format!("cannot remove {}", dir.display()))?;
    println!("removed {name} from the global catalogue");
    Ok(())
}

// ---------------------------------------------------------------------------
// Legacy single scope (`[apps] wasm_dir`)
// ---------------------------------------------------------------------------

/// List every component and whether the current allow-list publishes it.
fn list(dir: &Path, enabled: &[String]) -> Result<()> {
    let found = discover(dir, &[])?;
    if found.is_empty() {
        println!("no WASM apps in {}", dir.display());
        return Ok(());
    }
    let all = enabled.is_empty();
    for app in found {
        let on = all || enabled.iter().any(|name| name == &app.name);
        let size = fs::metadata(&app.path).map_or(0, |meta| meta.len());
        println!(
            "{}  {:<24} {:>8} B  {}",
            if on { "enabled " } else { "disabled" },
            app.name,
            size,
            app.path.display()
        );
    }
    for name in enabled {
        if !dir.join(format!("{name}.wasm")).is_file()
            && !dir.join(name).join(format!("{name}.wasm")).is_file()
        {
            println!("missing   {name:<24} enabled in [apps] but no component on disk");
        }
    }
    Ok(())
}

fn install(
    dir: &Path,
    config_path: &Path,
    enabled: &[String],
    source: &Path,
    name: Option<&str>,
) -> Result<()> {
    let bytes = fs::read(source)
        .with_context(|| format!("cannot read the component {}", source.display()))?;
    if !ferris_wasm::is_component(&bytes) {
        anyhow::bail!(
            "{} is not a Ferris component (a WebAssembly component of the `ferris-app` world)",
            source.display()
        );
    }
    let name = match name {
        Some(name) => name.to_string(),
        None => source
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow::anyhow!("cannot derive an app name from {}", source.display())
            })?,
    };
    if !is_valid_app_name(&name) {
        anyhow::bail!("invalid app name `{name}`: use letters, digits, `-` or `_` (at most 64)");
    }
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let target = dir.join(format!("{name}.wasm"));
    // Write-then-rename: a crash mid-copy never leaves a half component that
    // the next boot would try to compile.
    let temporary = dir.join(format!(".{name}.wasm.tmp"));
    fs::write(&temporary, &bytes)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    fs::rename(&temporary, &target)
        .with_context(|| format!("cannot publish {}", target.display()))?;
    ensure_world_at(dir, &name)?;
    if !enabled.is_empty() && !enabled.iter().any(|item| item == &name) {
        add_enabled(config_path, &name)?;
        println!("installed {name} and added it to [apps] enabled");
    } else {
        println!("installed {name} at {}", target.display());
    }
    Ok(())
}

fn remove(
    dir: &Path,
    config_path: &Path,
    enabled: &[String],
    name: &str,
    purge: bool,
) -> Result<()> {
    if !is_valid_app_name(name) {
        anyhow::bail!("invalid app name `{name}`");
    }
    let flat = dir.join(format!("{name}.wasm"));
    let nested = dir.join(name).join(format!("{name}.wasm"));
    let mut removed = false;
    for path in [&flat, &nested] {
        if path.is_file() {
            fs::remove_file(path).with_context(|| format!("cannot remove {}", path.display()))?;
            removed = true;
        }
    }
    let world = dir.join(name);
    if purge {
        if world.is_dir() {
            fs::remove_dir_all(&world)
                .with_context(|| format!("cannot remove {}", world.display()))?;
            removed = true;
            println!("purged the world of {name} (data, files and secrets)");
        }
    } else if world.is_dir() {
        println!(
            "kept the world at {}; use --purge to delete its data",
            world.display()
        );
    }
    if enabled.iter().any(|item| item == name) {
        remove_enabled(config_path, name)?;
        println!("removed {name} from [apps] enabled");
    }
    if !removed {
        anyhow::bail!("app `{name}` has no component in {}", dir.display());
    }
    println!("removed {name}");
    Ok(())
}

fn set_enabled(
    dir: &Path,
    config_path: &Path,
    enabled: &[String],
    name: &str,
    wanted: bool,
) -> Result<()> {
    if !is_valid_app_name(name) {
        anyhow::bail!("invalid app name `{name}`");
    }
    if wanted {
        if enabled.is_empty() {
            println!(
                "{name} is already enabled: `[apps] enabled` is empty, which publishes every component"
            );
            return Ok(());
        }
        if enabled.iter().any(|item| item == name) {
            println!("{name} is already enabled");
            return Ok(());
        }
        if !dir.join(format!("{name}.wasm")).is_file()
            && !dir.join(name).join(format!("{name}.wasm")).is_file()
        {
            anyhow::bail!(
                "no component for `{name}` in {}; run `ferris app install` first",
                dir.display()
            );
        }
        add_enabled(config_path, name)?;
        println!("enabled {name}");
        return Ok(());
    }

    // Empty means every component. Materialise that set first, so the list
    // really describes what is published from now on.
    let mut next: Vec<String> = if enabled.is_empty() {
        discover(dir, &[])?
            .into_iter()
            .map(|app| app.name)
            .collect()
    } else {
        enabled.to_vec()
    };
    if !next.iter().any(|item| item == name) {
        println!("{name} is already disabled");
        return Ok(());
    }
    next.retain(|item| item != name);
    write_enabled(config_path, &next)?;
    println!("disabled {name}");
    Ok(())
}

fn add_enabled(config_path: &Path, name: &str) -> Result<()> {
    let mut doc = read_document(config_path)?;
    let mut list = enabled_array(&doc);
    if !list.iter().any(|item| item == name) {
        list.push(name.to_string());
    }
    write_array(&mut doc, &list)?;
    save(config_path, &doc)
}

fn remove_enabled(config_path: &Path, name: &str) -> Result<()> {
    let mut doc = read_document(config_path)?;
    let mut list = enabled_array(&doc);
    list.retain(|item| item != name);
    write_array(&mut doc, &list)?;
    save(config_path, &doc)
}

fn write_enabled(config_path: &Path, names: &[String]) -> Result<()> {
    let mut doc = read_document(config_path)?;
    write_array(&mut doc, names)?;
    save(config_path, &doc)
}

fn read_document(path: &Path) -> Result<DocumentMut> {
    if !path.exists() {
        anyhow::bail!(
            "config file not found: {}. Run this from a project root or pass --config.",
            path.display()
        );
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    text.parse::<DocumentMut>()
        .map_err(|error| anyhow::anyhow!("{} is not valid TOML: {error}", path.display()))
}

fn enabled_array(doc: &DocumentMut) -> Vec<String> {
    doc.get("apps")
        .and_then(Item::as_table)
        .and_then(|apps| apps.get("enabled"))
        .and_then(Item::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn write_array(doc: &mut DocumentMut, names: &[String]) -> Result<()> {
    let mut array = Array::new();
    for name in names {
        array.push(name.as_str());
    }
    let apps = doc
        .entry("apps")
        .or_insert_with(|| Item::Table(Table::new()));
    let table = apps
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("`[apps]` in the config is not a table"))?;
    table["enabled"] = toml_edit::value(array);
    Ok(())
}

fn save(path: &Path, doc: &DocumentMut) -> Result<()> {
    fs::write(path, doc.to_string()).with_context(|| format!("cannot write {}", path.display()))
}
/// The path a world would occupy (used by the tests).
#[cfg(test)]
fn world(dir: &Path, name: &str) -> std::path::PathBuf {
    dir.join(name)
}

#[cfg(test)]
mod tests;
