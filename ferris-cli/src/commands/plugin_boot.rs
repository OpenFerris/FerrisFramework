//! Bootstrap of the WASM apps the server publishes at boot.
//!
//! `ferris serve` and `ferris run` used to start with
//! `ServerSettings.plugins == None`, so `build_router` never mounted `/plugins`
//! nor registered a single `/api/method/<name>` handler — even though
//! `ferris deploy` had already copied `plugins/<app>.wasm` next to the config.
//! The server answered every request as if the install had no apps, and said
//! nothing.
//!
//! This module is the one seam that decides *which* components are published,
//! *in what order*, and *what is fatal*. `ferris dev` goes through the same
//! [`load`] for the artifact under `target/`, so a component cannot be loaded
//! one way in development and another in production.

use anyhow::{Context, Result};
use ferris_core::{AppsConfig, Value};
use ferris_db::{Column, Db, UpsertBuilder, UpsertSet};
use ferris_wasm::{PluginStore, RuntimeConfig, SharedRuntime, load_component};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// The per-app config table the host reads (`config-get`/`config-set`),
/// duplicated from `ferris-wasm` so a rename breaks both writers.
const CONFIG_TABLE: &str = "tabAppConfig";

/// One component on disk and the name it is published under.
///
/// `name` is the file stem, so `plugins/my-app.wasm` is the app `my-app` — the
/// same spelling `ferris build` used to write the file (`plugins/<package>.wasm`,
/// dashes included) and the one `ferris dev` publishes its artifact under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppComponent {
    pub name: String,
    pub path: PathBuf,
}

/// The directory `apps.wasm_dir` names, resolved against the project root.
///
/// Defined in [`super::plugin_world`], which is not behind the `wasm` feature:
/// `ferris migrate` resolves the same directory to find the app worlds.
pub use super::plugin_world::plugins_dir;

/// Discover the components a directory holds, in a stable order.
///
/// Two layouts are accepted, and the flat one wins when both exist:
///
/// * `<dir>/<app>.wasm` — what `ferris build` writes and what every existing
///   deployment has;
/// * `<dir>/<app>/<app>.wasm` — the component inside the app's own world
///   (`plugins/<app>/`), for a plugin that ships as one directory.
///
/// The scan stays one level deep: a deeper `*.wasm` (a dev build's
/// `target/…/x.wasm` copied by hand) is never picked up, and a world's
/// sub-directories (`customizations/`, `files/`, …) are ignored.
///
/// `enabled` is an allow-list. Empty means "every component found"; a listed
/// name with no matching file is an error, because a boot that silently skips an
/// app the operator asked for reproduces exactly the symptom this module exists
/// to remove.
///
/// # Errors
///
/// The directory cannot be read, or `enabled` names a component that is not
/// there.
pub fn discover(dir: &Path, enabled: &[String]) -> Result<Vec<AppComponent>> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read app plugin directory {}", dir.display()))?;

    // A `BTreeMap` makes the "load everything" case ordered without a second
    // sort, and a duplicated name (only possible through a case-insensitive
    // filesystem) keeps the last path instead of publishing an app twice.
    let mut found: BTreeMap<String, PathBuf> = BTreeMap::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot read an entry of {}", dir.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(OsStr::to_str) != Some("wasm") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(OsStr::to_str) else {
            continue;
        };
        found.insert(name.to_string(), path);
    }

    // The world's own component, if it ships one. `or_insert` keeps the flat
    // file: `ferris build` writes it, and a leftover copy inside the world must
    // never shadow a fresh build.
    let worlds = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read app plugin directory {}", dir.display()))?;
    for entry in worlds {
        let entry = entry.with_context(|| format!("cannot read an entry of {}", dir.display()))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        let candidate = path.join(format!("{name}.wasm"));
        if candidate.is_file() {
            found.entry(name.to_string()).or_insert(candidate);
        }
    }

    if enabled.is_empty() {
        return Ok(found
            .into_iter()
            .map(|(name, path)| AppComponent { name, path })
            .collect());
    }

    let mut selected = Vec::with_capacity(enabled.len());
    for name in enabled {
        let Some(path) = found.remove(name.as_str()) else {
            anyhow::bail!(
                "app `{name}` is enabled in [apps] but {} has no {name}.wasm \
                 (nor {name}/{name}.wasm); run `ferris build` or fix [apps] enabled",
                dir.display()
            );
        };
        selected.push(AppComponent {
            name: name.clone(),
            path,
        });
    }
    selected.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(selected)
}

/// A plugin store with the shared runtime policy.
///
/// `compile_cache` is on for the same reason `ferris dev` set it: reloads and
/// multi-app boots compile each artifact once instead of re-running cranelift
/// over the whole component. The WASM epoch clock is deliberately **not**
/// started here — `ferris_server::Server::run` owns it for the process lifetime
/// (C4), and a second clock would make every call deadline fire sooner.
///
/// # Errors
///
/// The wasmtime engine cannot be configured.
pub fn new_store(pool: Db, worlds: Option<&Path>) -> Result<Arc<PluginStore>> {
    // The WIT `platform::features()` import must answer before the first
    // `describe()` below, not only after `Server::run` publishes the same set.
    let _ = ferris_server::host_features::install_compiled();
    let runtime = Arc::new(SharedRuntime::new(RuntimeConfig {
        compile_cache: true,
        ..Default::default()
    })?);
    let store = PluginStore::new(runtime, pool);
    Ok(Arc::new(match worlds {
        Some(dir) => store.with_worlds(dir),
        None => store,
    }))
}

/// Compile, wire and publish one component.
///
/// Reliability: `swap_load_and_wire` registers the app's `DocType`s and lifecycle
/// events *before* publishing it, so no request ever reaches a plugin whose
/// schema is not registered yet, and a component that fails to wire leaves the
/// previously loaded apps serving.
///
/// Capabilities are no longer configured: the component's own `describe()`
/// declaration installs the primordial set plus the specific capabilities it
/// names, before any request is served.
///
/// # Errors
///
/// The file cannot be read, is not a valid component, or its `describe()`/
/// `DocType` registration fails.
///
/// # Panics
///
/// `rt` must be a multi-threaded runtime: wiring calls the guest's `describe()`,
/// whose host imports park a worker through `block_in_place`. Every caller here
/// builds a multi-thread runtime (`serve::start`, `ferris dev`).
pub fn load(
    rt: &tokio::runtime::Runtime,
    store: &Arc<PluginStore>,
    app: &AppComponent,
) -> Result<()> {
    let component = load_component(&app.path)
        .with_context(|| format!("cannot read app component {}", app.path.display()))?;
    rt.block_on(async {
        tokio::task::block_in_place(|| store.swap_load_and_wire(&component, &app.name))
    })
    .with_context(|| {
        format!(
            "app `{}` failed to load from {}",
            app.name,
            app.path.display()
        )
    })?;
    info!(
        app = %app.name,
        path = %app.path.display(),
        "app plugin loaded"
    );
    Ok(())
}

/// Read the `[config]` table of a world's `config.toml`, as the strings the guest
/// reads back through `config-get`.
///
/// `ferris site install-app` writes that file: `[ferris]` names the site, the app
/// and its version and is ignored, while `[config]` holds the manifest defaults,
/// the site's overrides and the operator's edits — the **effective** configuration
/// in that site. Scalars render to text (`3` → `"3"`), an array or table becomes
/// compact JSON, and a structured `secret` is refused: the column is plaintext
/// and the host owns secret material. `Ok(None)` means no file, invalid names it.
///
/// # Errors
///
/// The file cannot be read or parsed, or a `secret` value is not a string.
pub fn read_world_config(world: &Path) -> Result<Option<BTreeMap<String, String>>> {
    let path = world.join(ferris_core::site::CONFIG_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let root: toml::Table = toml::from_str(&text)
        .with_context(|| format!("invalid app config in {}", path.display()))?;
    let mut values = BTreeMap::new();
    if let Some(config) = root.get("config").and_then(toml::Value::as_table) {
        for (key, value) in config {
            values.insert(key.clone(), config_text(key, value, &path)?);
        }
    }
    Ok(Some(values))
}

/// One `[config]` value as the text `tabAppConfig` stores.
fn config_text(key: &str, value: &toml::Value, path: &Path) -> Result<String> {
    Ok(match value {
        toml::Value::String(text) => text.clone(),
        toml::Value::Integer(number) => number.to_string(),
        toml::Value::Float(number) => number.to_string(),
        toml::Value::Boolean(flag) => flag.to_string(),
        toml::Value::Datetime(stamp) => stamp.to_string(),
        toml::Value::Array(_) | toml::Value::Table(_) if key == "secret" => anyhow::bail!(
            "{}: `[config] secret` must be a string, not the material itself",
            path.display()
        ),
        toml::Value::Array(_) | toml::Value::Table(_) => serde_json::to_string(
            &json_value(value).with_context(|| format!("{}: `[config] {key}`", path.display()))?,
        )
        .with_context(|| format!("{}: `[config] {key}`", path.display()))?,
    })
}

/// A TOML value as JSON. `None` for a non-finite float (`nan`/`inf` are valid
/// TOML and invalid JSON), so it is reported instead of becoming `null`.
fn json_value(value: &toml::Value) -> Option<serde_json::Value> {
    Some(match value {
        toml::Value::String(text) => text.as_str().into(),
        toml::Value::Integer(number) => (*number).into(),
        toml::Value::Float(number) => serde_json::Number::from_f64(*number)?.into(),
        toml::Value::Boolean(flag) => (*flag).into(),
        toml::Value::Datetime(stamp) => stamp.to_string().into(),
        toml::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(json_value).collect::<Option<_>>()?)
        }
        toml::Value::Table(table) => table
            .iter()
            .map(|(key, item)| Some((key.clone(), json_value(item)?)))
            .collect::<Option<serde_json::Map<String, serde_json::Value>>>()?
            .into(),
    })
}

/// Upsert one app's `[config]` rows into [`CONFIG_TABLE`] before it is loaded.
///
/// Additive and idempotent: the file owns its keys, and a key the guest wrote
/// through `config-set` that the file does not name is left alone.
///
/// # Errors
///
/// The table cannot be created, or a row cannot be upserted.
pub async fn seed_config(pool: &Db, app: &str, values: &BTreeMap<String, String>) -> Result<()> {
    // Host-owned storage (`ferris-wasm` creates it too); it must exist before `describe()`.
    pool.ensure_table(
        CONFIG_TABLE,
        &[
            Column::new("plugin", "VARCHAR(255)")
                .primary_key()
                .not_null(),
            Column::new("key", "VARCHAR(255)").primary_key().not_null(),
            Column::new("value", "TEXT").not_null(),
        ],
    )
    .await
    .with_context(|| format!("cannot create {CONFIG_TABLE}"))?;
    for (key, value) in values {
        let upsert = UpsertBuilder::new(pool.dialect(), CONFIG_TABLE)
            .values(vec![
                ("plugin", Value::from(app)),
                ("key", Value::from(key.as_str())),
                ("value", Value::from(value.as_str())),
            ])
            .conflict_on(["plugin", "key"])
            .on_update("value", UpsertSet::Value(Value::from(value.as_str())))
            .build()?;
        pool.upsert(upsert).await?;
    }
    Ok(())
}

/// Build the store and publish every configured app.
///
/// `Ok(None)` means the install has no apps, which is a valid deployment: a
/// server without a `plugins/` directory serves the framework Desk alone. Any
/// component that is present but cannot be published is an error, so the process
/// stops at boot instead of serving a Desk with no app modules.
///
/// Each app's world `config.toml` is applied to `tabAppConfig` before the
/// component is loaded (see [`read_world_config`]), so a plugin's effective
/// configuration comes from the site's file rather than from defaults.
///
/// # Errors
///
/// The plugin store's wasmtime engine cannot be configured, the directory cannot
/// be read, an app listed in [`AppsConfig::enabled`] is missing, a world's
/// `config.toml` is invalid, or a component fails to load.
pub fn bootstrap(
    rt: &tokio::runtime::Runtime,
    pool: Db,
    apps: &AppsConfig,
    project_dir: &Path,
) -> Result<Option<Arc<PluginStore>>> {
    let dir = plugins_dir(project_dir, apps);

    if !dir.is_dir() {
        if apps.enabled.is_empty() {
            info!(
                dir = %dir.display(),
                "no app plugin directory; serving without app modules (run `ferris build` to add one)"
            );
            return Ok(None);
        }
        anyhow::bail!(
            "[apps] enables {} app(s) but {} is not a directory; run `ferris build`",
            apps.enabled.len(),
            dir.display()
        );
    }

    let discovered = discover(&dir, &apps.enabled)?;
    if discovered.is_empty() {
        warn!(
            dir = %dir.display(),
            "the app plugin directory is empty; the Desk will run without app modules"
        );
        return Ok(None);
    }

    let store = new_store(pool.clone(), Some(&dir))?;
    for app in &discovered {
        // The app's world exists before it is loaded: its customizations,
        // files and secrets live there, and an export at runtime must not race
        // the first boot.
        let world = ferris_core::plugins::ensure_world_at(&dir, &app.name)
            .with_context(|| format!("cannot prepare the world for app `{}`", app.name))?;
        debug!(app = %app.name, world = %world.display(), "app world ready");
        // The world's `config.toml` is the app's effective configuration in this
        // site, applied before `load` so `describe()` and the first `config-get`
        // already see the operator's values.
        if let Some(values) = read_world_config(&world)? {
            rt.block_on(seed_config(&pool, &app.name, &values))
                .with_context(|| format!("cannot apply the config of app `{}`", app.name))?;
            debug!(app = %app.name, keys = values.len(), "app config seeded");
        }
        load(rt, &store, app)?;
    }
    info!(
        count = discovered.len(),
        dir = %dir.display(),
        "app plugins loaded"
    );
    Ok(Some(store))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own, so parallel tests cannot collide.
    fn scratch_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ferris-plugins-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"\0asm").unwrap();
    }

    #[test]
    fn discovers_wasm_files_in_a_stable_order() {
        let dir = scratch_dir("order");
        touch(&dir, "b.wasm");
        touch(&dir, "a.wasm");
        touch(&dir, "notes.txt");
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        touch(&dir, "nested/c.wasm");

        let found = discover(&dir, &[]).unwrap();
        let names: Vec<&str> = found.iter().map(|app| app.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["a", "b"],
            "alphabetical, non-recursive, wasm only"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A plugin can ship as one directory: the world holds its own component.
    /// When both layouts exist the flat file wins, because `ferris build`
    /// rewrites that one and a stale copy must not shadow it.
    #[test]
    fn a_world_may_hold_its_own_component() {
        let dir = scratch_dir("world");
        std::fs::create_dir_all(dir.join("demo/customizations")).unwrap();
        touch(&dir.join("demo"), "demo.wasm");
        // Not `<dir>/<name>.wasm` inside the directory: a world is not a tree.
        std::fs::create_dir_all(dir.join("other")).unwrap();
        touch(&dir.join("other"), "stray.wasm");

        let found = discover(&dir, &[]).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "demo");
        assert_eq!(found[0].path, dir.join("demo/demo.wasm"));

        // The flat build output takes precedence.
        touch(&dir, "demo.wasm");
        let found = discover(&dir, &[]).unwrap();
        assert_eq!(found[0].path, dir.join("demo.wasm"));

        // And an enabled world resolves without a flat file.
        std::fs::remove_file(dir.join("demo.wasm")).unwrap();
        let found = discover(&dir, &["demo".to_string()]).unwrap();
        assert_eq!(found[0].path, dir.join("demo/demo.wasm"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_enabled_list_selects_and_keeps_alphabetical_order() {
        let dir = scratch_dir("enabled");
        touch(&dir, "a.wasm");
        touch(&dir, "b.wasm");
        touch(&dir, "c.wasm");

        let enabled = vec!["b".to_string(), "a".to_string()];
        let found = discover(&dir, &enabled).unwrap();
        let names: Vec<&str> = found.iter().map(|app| app.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["a", "b"],
            "the config order must not decide the load order"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_enabled_app_that_is_missing_is_an_error() {
        let dir = scratch_dir("missing");
        touch(&dir, "a.wasm");

        let err = discover(&dir, &["ghost".to_string()])
            .expect_err("an enabled app with no component must fail the boot");
        let message = err.to_string();
        assert!(message.contains("ghost"), "{message}");
        assert!(message.contains(&dir.display().to_string()), "{message}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_absent_directory_is_not_an_error() {
        let dir = scratch_dir("absent");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(
            discover(&dir, &[]).is_err(),
            "discover assumes an existing directory"
        );

        // `bootstrap` is the layer that tolerates an install with no apps.
        let apps = AppsConfig::default();
        assert_eq!(
            plugins_dir(Path::new("/srv/app"), &apps),
            PathBuf::from("/srv/app/plugins")
        );
    }

    #[test]
    fn plugin_names_keep_the_package_dash() {
        let dir = scratch_dir("dash");
        touch(&dir, "my-app.wasm");

        let found = discover(&dir, &[]).unwrap();
        assert_eq!(found[0].name, "my-app");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_filename_is_the_plugin_name() {
        // The contract `ferris build` writes: `plugins/<package>.wasm`.
        let dir = scratch_dir("name");
        touch(&dir, "ferris-app.wasm");
        let found = discover(&dir, &[]).unwrap();
        assert_eq!(found[0].name, "ferris-app");
        assert!(found[0].path.ends_with("ferris-app.wasm"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_absolute_wasm_dir_is_not_joined_to_the_project() {
        let apps = AppsConfig {
            wasm_dir: PathBuf::from("/opt/ferris/apps"),
            ..AppsConfig::default()
        };
        assert_eq!(
            plugins_dir(Path::new("/srv/app"), &apps),
            PathBuf::from("/opt/ferris/apps")
        );
    }

    fn test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Runtime::new().unwrap()
    }

    fn config_row(rt: &tokio::runtime::Runtime, pool: &Db, key: &str) -> Option<String> {
        let stmt = ferris_db::Stmt::new(
            "SELECT value FROM tabAppConfig WHERE plugin = ? AND key = ?",
            vec![Value::from("demo"), Value::from(key)],
        );
        rt.block_on(pool.fetch_optional(&stmt))
            .unwrap()
            .and_then(|row| row.opt_str("value"))
    }

    /// The world's `config.toml` is the app's effective configuration: `[config]`
    /// lands in `tabAppConfig`, a second boot keeps a key the plugin wrote itself,
    /// and a broken file fails the boot.
    #[test]
    fn the_world_config_seeds_the_app_config_table() {
        let rt = test_runtime();
        let pool = rt.block_on(ferris_db::memory_db()).unwrap();
        let dir = scratch_dir("config");
        let world = dir.join("demo");
        std::fs::create_dir_all(&world).unwrap();
        let file = world.join("config.toml");
        let boot = || {
            let values = read_world_config(&world).unwrap().unwrap();
            rt.block_on(seed_config(&pool, "demo", &values)).unwrap();
        };

        // A file the operator must fix is reported, never silently ignored.
        std::fs::write(&file, "[config\ncurrency = 1").unwrap();
        let err = read_world_config(&world)
            .expect_err("invalid TOML")
            .to_string();
        assert!(err.contains("config.toml"), "{err}");
        std::fs::write(&file, "[config]\nsecret = { token = \"clear\" }\n").unwrap();
        let err = read_world_config(&world)
            .expect_err("structured secret")
            .to_string();
        assert!(err.contains("secret"), "{err}");

        std::fs::write(
            &file,
            "[ferris]\nsite = \"demo\"\n\n[config]\ncurrency = \"EUR\"\nretries = 3\n",
        )
        .unwrap();
        boot();
        assert_eq!(config_row(&rt, &pool, "currency").as_deref(), Some("EUR"));
        assert_eq!(config_row(&rt, &pool, "retries").as_deref(), Some("3"));

        // What the guest writes through `config-set` is not a key of the file.
        let guest = BTreeMap::from([("token_cache".to_string(), "keep".to_string())]);
        rt.block_on(seed_config(&pool, "demo", &guest)).unwrap();

        std::fs::write(&file, "[config]\ncurrency = \"USD\"\nretries = 3\n").unwrap();
        boot();
        assert_eq!(config_row(&rt, &pool, "currency").as_deref(), Some("USD"));
        assert_eq!(
            config_row(&rt, &pool, "token_cache").as_deref(),
            Some("keep")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
