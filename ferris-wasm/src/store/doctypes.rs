//! Schema registration and engine-hook wiring for a declared app.

use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{info, warn};

use ferris_document::engine::DocumentEngine;
use ferris_document::naming::NamingSeriesStore;
use ferris_schema::try_register_app_doctype;

use super::hooks::{EVENT_POINTS, register_event_hook};
use super::registry::PluginStore;
use crate::Plugin;
use crate::descriptor;
use crate::ferris::framework::descriptor::Descriptor;

/// Validate one declared `DocType` and append it to the registration batch.
///
/// Shared by the descriptor subset and `schema::declare-doctype`, so an app
/// cannot bypass the canonical fieldtype or ownership checks by choosing the
/// other surface.
fn prepare_meta(
    module: &str,
    meta: ferris_schema::DoctypeMeta,
    prepared: &mut Vec<(ferris_schema::DoctypeMeta, String)>,
) -> Result<()> {
    ferris_schema::registry::validate_app_doctype(module, &meta)
        .with_context(|| format!("app {module} cannot register DocType {}", meta.name))?;
    // The meta is the fingerprint: a DocType edited in the app while the dev
    // server runs must re-register, otherwise the schema the engine uses stays
    // frozen at whatever the first reload declared.
    let fingerprint = serde_json::to_string(&meta)?;
    prepared.push((meta, fingerprint));
    Ok(())
}

impl PluginStore {
    /// Consume `describe()`: register the advertised `DocTypes` with the engine
    /// and the schema registry so the framework knows them. Also registers any
    /// `"themes"` the app declared (the extensible theme registry in
    /// `ferris-core`). Idempotent, so it is safe to call again after a hot
    /// reload. Returns the `DocTypes` that were wired on this call.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded or wiring the declared schema
    /// fails.
    pub fn register_doctypes(&self) -> Result<Vec<String>> {
        let plugins = self.plugins();
        if plugins.is_empty() {
            anyhow::bail!("no plugin loaded");
        }
        let mut wired = Vec::new();
        for plugin in plugins {
            let described = self
                .guard_trap(plugin.describe(), plugin.name())
                .map_err(anyhow::Error::from)?;
            let declared = plugin.take_declared_doctypes();
            wired.extend(self.register_doctypes_of(plugin.name(), &described, &declared)?);
        }
        Ok(wired)
    }

    /// As [`Self::register_doctypes`], for a plugin that is not published yet.
    ///
    /// This is what makes a reload atomic: the new component's `DocTypes` reach
    /// the registry and the engine *before* it becomes the plugin requests are
    /// served from, so no request ever sees the two halves disagree.
    ///
    /// # Errors
    ///
    /// Returns an error when the plugin's `describe()` call fails or a schema
    /// registration fails.
    #[allow(clippy::significant_drop_tightening)] // `synced` is the dedup guard for the whole loop: each iteration reads and writes it.
    pub fn register_doctypes_of(
        &self,
        module: &str,
        described: &Descriptor,
        declared: &[String],
    ) -> Result<Vec<String>> {
        let module = module.to_string();

        // Validate the **whole** descriptor before any side effect. Themes,
        // workspaces and UI extensions are registered below; if a later DocType
        // were refused, a reload would have left those changes applied without
        // the schema they belong to. Building and checking the metadata first
        // also lets the strict fieldtype allowlist fail with the exact field.
        let mut prepared = Vec::with_capacity(described.doctypes.len());
        for dt in &described.doctypes {
            let meta = descriptor::doctype_meta(dt, &module);
            prepare_meta(&module, meta, &mut prepared)?;
        }
        // The escape hatch: `schema::declare-doctype` staged the full metadata
        // during `describe()`. Validate and register it through the same path as
        // a descriptor declaration, so the two cannot disagree about ownership,
        // reserved names or fieldtypes.
        for definition in declared {
            let meta: ferris_schema::DoctypeMeta = serde_json::from_str(definition)
                .with_context(|| format!("app {module} declared an invalid DocType JSON"))?;
            prepare_meta(&module, meta, &mut prepared)?;
        }

        let theme_registration = ferris_core::themes::register_themes(
            described
                .themes
                .iter()
                .map(ferris_core::themes::ThemeSpec::from)
                .collect(),
        );
        for warning in &theme_registration.warnings {
            warn!("themes ({module}): {warning}");
        }

        let workspace_registration = ferris_core::workspaces::register_app_workspaces(
            &module,
            described
                .folders
                .iter()
                .map(ferris_core::workspaces::WorkspaceFolderSpec::from)
                .collect(),
            described
                .workspaces
                .iter()
                .map(descriptor::workspace_spec)
                .collect(),
        );
        for warning in &workspace_registration.warnings {
            warn!("workspaces ({module}): {warning}");
        }

        let ui_registration = ferris_core::ui_extensions::register_app_ui(
            &module,
            ferris_core::ui_extensions::AppUiSpec {
                extensions: described
                    .ui_extensions
                    .iter()
                    .flatten()
                    .map(descriptor::ui_extension_spec)
                    .collect(),
                dashboards: described
                    .dashboards
                    .iter()
                    .flatten()
                    .map(descriptor::dashboard_spec)
                    .collect(),
                widgets: described
                    .widgets
                    .iter()
                    .flatten()
                    .map(descriptor::widget_def_spec)
                    .collect(),
                website: described.website.as_deref().map(descriptor::website_spec),
                assets: described.assets.clone().unwrap_or_default(),
            },
        );
        for warning in &ui_registration.warnings {
            warn!("ui extensions ({module}): {warning}");
        }

        let mut wired = Vec::new();
        let mut synced = self
            .synced
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if synced.is_empty() {
            // First registration: make sure naming series work on a fresh DB.
            crate::setup::block_on(NamingSeriesStore::ensure_table(&self.pool))?;
        }
        for (meta, fingerprint) in prepared {
            let name = meta.name.clone();
            let key = (module.clone(), name.clone());
            if synced.get(&key).is_some_and(|seen| *seen == fingerprint) {
                continue;
            }
            try_register_app_doctype(&module, meta)
                .with_context(|| format!("app {module} cannot register DocType {name}"))?;
            let engine = DocumentEngine::new(self.pool.clone());
            crate::setup::block_on(engine.ensure_table(&name))?;
            synced.insert(key, fingerprint);
            info!(doctype = %name, module = %module, "app DocType registered");
            wired.push(name);
        }
        Ok(wired)
    }

    /// Wire the plugin's `DocTypes` into the engine lifecycle: every create /
    /// save / submit / cancel / trash of those `DocTypes` is forwarded to the
    /// plugin's `doctype_event` export. Idempotent across hot reloads.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded or wiring fails.
    pub fn wire_engine_events(self: &Arc<Self>) -> Result<()> {
        self.start_drainer();
        let plugin = self
            .loaded()
            .ok_or_else(|| anyhow::anyhow!("no plugin loaded"))?;
        self.wire_events_of(&plugin)
    }

    /// Wire the lifecycle hooks of a plugin that is not published yet. The
    /// `describe()` call here is deliberately **not** trap-guarded: this plugin
    /// is not published, so a trap must fail the reload cleanly (the previous
    /// plugin stays serving) instead of [`PluginStore::recover_after_trap`]
    /// swapping in a half-wired component.
    pub(super) fn wire_events_of(self: &Arc<Self>, plugin: &Plugin) -> Result<()> {
        let described = plugin.describe()?;
        let declared = plugin.take_declared_doctypes();
        for doctype in self.register_doctypes_of(plugin.name(), &described, &declared)? {
            for (point, event) in EVENT_POINTS {
                register_event_hook(self.clone(), &doctype, point, event);
            }
        }
        Ok(())
    }
}
