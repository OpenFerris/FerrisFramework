//! Apply the customizations a plugin keeps in its world.
//!
//! A plugin's `customizations/` directory is the app's copy of its metadata
//! changes: `POST /api/customization/{doctype}/export` writes
//! `plugins/<app>/customizations/<doctype>.json`, and `ferris migrate` reads
//! those files back into `tab_custom_field` / `tab_property_setter`.
//!
//! This is Frappe's fixtures model: the app owns the payload, the database is a
//! projection of it. Applying it on `migrate` (deploy time) rather than on
//! every boot is deliberate — a customization made in the Desk and **not** yet
//! exported must not be silently overwritten when the process restarts; the
//! operator decides when the app's copy wins.
//!
//! Both writes are upserts, so running the command twice is a no-op and
//! deleting a field from the world does not delete the row (that is an explicit
//! Desk action, not a side effect of a redeploy).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ferris_db::Db;
use ferris_schema::CustomizationStore;
use ferris_schema::contract::customization::WorldCustomization;
use ferris_schema::customization::{CustomField, PropertySetter};
use tracing::{debug, info};

/// The directory `apps.wasm_dir` names, resolved against the project root.
///
/// `project_dir` is the caller's project root (the resolved `--dir`, or the
/// working directory), not the framework checkout: the directory belongs to the
/// deployment. An absolute `wasm_dir` is used as-is.
///
/// Lives here rather than in `plugin_boot` because `ferris migrate` — which is
/// also built without the `wasm` feature — resolves it to find the worlds.
#[must_use]
pub fn plugins_dir(project_dir: &Path, apps: &ferris_core::AppsConfig) -> PathBuf {
    if apps.wasm_dir.is_absolute() {
        return apps.wasm_dir.clone();
    }
    // `apps.wasm_dir` defaults to `./plugins`; joining it verbatim would put
    // `/srv/app/./plugins` in every log line and error message. Drop the `.`
    // components and let the project root speak for itself.
    let relative: PathBuf = apps
        .wasm_dir
        .components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .collect();
    project_dir.join(relative)
}

/// Read every `customizations/*.json` of one world, in file-name order.
///
/// # Errors
///
/// A file that cannot be read or parsed. The error names the file, because the
/// payload carries no provenance of its own.
pub fn read_world_customizations(world: &Path) -> Result<Vec<WorldCustomization>> {
    let dir = world.join("customizations");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(std::ffi::OsStr::to_str) == Some("json"))
        .collect();
    files.sort();

    let mut found = Vec::with_capacity(files.len());
    for path in files {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let customization: WorldCustomization = serde_json::from_str(&text)
            .with_context(|| format!("invalid customization in {}", path.display()))?;
        found.push(customization);
    }
    Ok(found)
}

/// Upsert one world's customizations into the database.
///
/// Returns how many fields and properties were applied.
///
/// # Errors
///
/// A malformed file, an unknown fieldtype, or a database error. Nothing is
/// silently skipped: a file the app ships must apply or fail the migration.
pub async fn apply_world_customizations(pool: &Db, world: &Path) -> Result<usize> {
    let customizations = read_world_customizations(world)?;
    if customizations.is_empty() {
        return Ok(0);
    }
    let store = CustomizationStore::with_db(pool.clone());
    store.init().await?;

    let mut applied = 0;
    for entry in customizations {
        let doctype = entry.doctype.clone();
        if doctype.trim().is_empty() {
            anyhow::bail!(
                "a customization in {} has no doctype; re-export it from the Desk",
                world.display()
            );
        }
        for spec in entry.customization.fields {
            store
                .add_custom_field(CustomField {
                    doctype: doctype.clone(),
                    fieldname: spec.fieldname,
                    label: spec.label,
                    fieldtype: spec.fieldtype,
                    options: spec.options,
                    insert_after: spec.insert_after,
                    reqd: spec.reqd,
                    default: spec.default,
                    hidden: spec.hidden,
                    read_only: spec.read_only,
                })
                .await?;
            applied += 1;
        }
        for spec in entry.customization.properties {
            store
                .set_property(PropertySetter {
                    doctype: doctype.clone(),
                    fieldname: spec.fieldname,
                    property: spec.property,
                    value: spec.value,
                })
                .await?;
            applied += 1;
        }
        debug!(%doctype, world = %world.display(), "plugin customization applied");
    }
    Ok(applied)
}

/// Apply the worlds of every enabled app (or every world, when `[apps]` does
/// not name any), returning how many entries were applied.
///
/// A world without a `customizations/` directory is skipped; it belongs to an
/// app that has nothing to re-apply.
///
/// # Errors
///
/// A world that cannot be read or applied, or a `plugins/` directory that
/// cannot be listed.
pub async fn apply_all(pool: &Db, plugins: &Path, enabled: &[String]) -> Result<usize> {
    let entries = match std::fs::read_dir(plugins) {
        Ok(entries) => entries,
        // No plugin directory is not a failure: a framework-only deployment has
        // none, and `ferris migrate` must still work.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(e).with_context(|| format!("cannot read {}", plugins.display()));
        }
    };

    let mut worlds: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .filter(|path| {
            path.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(ferris_core::plugins::is_valid_app_name)
        })
        .collect();
    worlds.sort();

    let mut applied = 0;
    for world in worlds {
        let Some(app) = world.file_name().and_then(std::ffi::OsStr::to_str) else {
            continue;
        };
        if !enabled.is_empty() && !enabled.iter().any(|name| name.as_str() == app) {
            continue;
        }
        let count = apply_world_customizations(pool, &world).await?;
        if count > 0 {
            info!(app, count, "plugin customizations applied");
        }
        applied += count;
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ferris-world-customizations-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_world(world: &Path, doctype: &str) {
        let dir = world.join("customizations");
        std::fs::create_dir_all(&dir).unwrap();
        // The file name is the sanitized stem; the doctype travels inside.
        let payload = format!(
            r#"{{
              "doctype": "{doctype}",
              "fields": [
                {{"fieldname": "note", "label": "Note", "fieldtype": "Data",
                 "insert_after": "title", "reqd": false, "hidden": false,
                 "read_only": false}}
              ],
              "properties": [
                {{"fieldname": "note", "property": "placeholder", "value": "here"}}
              ]
            }}"#
        );
        std::fs::write(dir.join("Custom-Doc.json"), payload).unwrap();
    }

    #[test]
    fn a_world_is_read_back_with_its_doctype() {
        let root = scratch_dir("read");
        write_world(&root.join("demo"), "Task Line");

        let found = read_world_customizations(&root.join("demo")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].doctype, "Task Line");
        assert_eq!(found[0].customization.fields[0].fieldname, "note");

        // A world that ships nothing is not an error.
        assert!(
            read_world_customizations(&root.join("empty"))
                .unwrap()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_malformed_file_names_itself() {
        let root = scratch_dir("bad");
        let dir = root.join("demo/customizations");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Broken.json"), "{not json").unwrap();

        let err = read_world_customizations(&root.join("demo")).unwrap_err();
        assert!(
            err.to_string().contains("Broken.json"),
            "the error has to identify the file: {err}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn applying_a_world_is_idempotent() {
        let pool = ferris_db::memory_db().await.unwrap();
        let root = scratch_dir("apply");
        write_world(&root.join("demo"), "Task Line");

        let first = apply_world_customizations(&pool, &root.join("demo"))
            .await
            .unwrap();
        assert_eq!(first, 2, "one field and one property");
        // Twice: the upserts replace instead of duplicating.
        let second = apply_world_customizations(&pool, &root.join("demo"))
            .await
            .unwrap();
        assert_eq!(second, 2);

        let store = CustomizationStore::with_db(pool.clone());
        let fields = store.custom_fields("Task Line").await.unwrap();
        assert_eq!(fields.len(), 1);
        let setters = store.property_setters("Task Line").await.unwrap();
        assert_eq!(setters.len(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn only_enabled_apps_are_applied() {
        let pool = ferris_db::memory_db().await.unwrap();
        let root = scratch_dir("enabled");
        write_world(&root.join("demo"), "Demo Doc");
        write_world(&root.join("other"), "Other Doc");

        let applied = apply_all(&pool, &root, &["other".to_string()])
            .await
            .unwrap();
        assert_eq!(applied, 2, "only `other`");

        let store = CustomizationStore::with_db(pool.clone());
        assert!(
            store.custom_fields("Demo Doc").await.unwrap().is_empty(),
            "the disabled app was not applied"
        );
        assert_eq!(store.custom_fields("Other Doc").await.unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
