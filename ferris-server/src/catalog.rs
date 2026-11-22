//! The host's DocType catalog.
//!
//! Every crate declares its own DocTypes (in its own `doctypes` module); the
//! host only lists the crates, in a deterministic order, and calls them. Nothing
//! about a DocType is centralized here — this is wiring, not definition
//!.
//!
//! [`register_doctype_catalog`] must run **before** any workspace registration,
//! because a workspace validates its items against DocTypes that already exist.
//!
//! # Compile-time features
//!
//! A vertical that was not compiled has no crate to call, so its DocTypes and
//! workspaces are absent from the catalog. That is the honest projection: the
//! Desk would otherwise show a workspace whose every link 404s. The always-on
//! owners (`schema`, `api`, `document`, `auth`, the server itself) are called
//! unconditionally because `api` is the base every build carries.
#![allow(clippy::doc_markdown)]

/// Register every native crate's DocTypes, in order.
///
/// Idempotent: each crate overwrites its own keys, so calling this on every boot
/// (and after a hot reload) is safe.
pub fn register_doctype_catalog(modules: &ferris_api::ModuleRegistry) {
    // The schema crate owns the registry and (later) the framework metadata
    // DocTypes.
    ferris_schema::doctypes::register_doctypes();
    // The Desk shell's framework and Data DocTypes (Notification, Event,
    // Notification Settings, Desk Layout, Data Import/Export, naming, reports)
    // — ex `register_core_doctypes`.
    ferris_api::doctypes::register_doctypes();
    // The schema-of-schema DocTypes (`DocType`, `DocField`), owned by the
    // document engine crate.
    ferris_document::doctypes::register_doctypes();
    // The identity and access-control surface (Users).
    ferris_auth::doctypes::register_doctypes();
    // The CMS/public-site surface (pages, web forms, website/portal settings).
    #[cfg(feature = "web")]
    ferris_web::doctypes::register_doctypes();
    // The unified server's System DocTypes.
    crate::doctypes::register_doctypes();
    // Optional verticals own their declarations: the registry is the only list
    // of features, so a module that is not registered contributes nothing.
    for module in modules.modules() {
        module.register_doctypes();
    }
}

/// Register the DocTypes of every compiled module, independent of the runtime
/// switches.
///
/// Used by `ferris doctor` to inspect the schema of a binary without booting a
/// server; an install may have `[features] print = false`, but the binary still
/// carries the declarations and the doctor should report them.
pub fn register_compiled_doctype_catalog() {
    let modules = crate::router::modules::native_modules(&ferris_core::FeatureFlags::default());
    register_doctype_catalog(&modules);
}

/// Register every native crate's Desk workspaces, in order, and log the result.
///
/// The host does not enumerate items — each crate declares its own workspace in
/// its own `workspaces` module and this only calls them, exactly like
/// [`register_doctype_catalog`]. Must run **after**
/// the DocType catalog so the start-up diagnostics can resolve every declared
/// doctype link against the full registry; the `ferris-api` filter that keeps
/// dead links out of the sidebar is `ferris_api::desk`.
///
/// Idempotent per owner: each crate replaces its own previous layout, so calling
/// this on every boot (and after a hot reload) is safe.
pub fn register_workspace_catalog(modules: &ferris_api::ModuleRegistry) {
    // Built as a list instead of two parallel arrays so a compiled-out vertical
    // removes its owner and its registration in one place: the zip can never
    // pair the wrong owner with the wrong set.
    let mut registered = Vec::new();
    registered.push((
        "ferris-schema",
        ferris_schema::workspaces::register_workspaces(),
    ));
    registered.push((
        "ferris-auth",
        ferris_auth::workspaces::register_workspaces(),
    ));
    #[cfg(feature = "web")]
    registered.push(("ferris-web", ferris_web::workspaces::register_workspaces()));
    registered.push(("ferris-api", ferris_api::workspaces::register_workspaces()));
    registered.push(("ferris-server", crate::workspaces::register_workspaces()));

    for (owner, registration) in registered {
        for warning in &registration.warnings {
            tracing::warn!("workspace declaration ({owner}): {warning}");
        }
        tracing::info!("workspaces registered by {owner}: {:?}", registration.names);
    }
    // Optional verticals register their own workspaces; the module name is the
    // owner in the warning, so a bad declaration still names the crate that
    // produced it.
    for module in modules.modules() {
        let registration = module.register_workspaces();
        for warning in &registration.warnings {
            tracing::warn!("workspace declaration ({}): {warning}", module.name());
        }
        if !registration.names.is_empty() {
            tracing::info!(
                "workspaces registered by {}: {:?}",
                module.name(),
                registration.names
            );
        }
    }
    // The sidebar hides a declared link whose DocType is not registered; name
    // those once here instead of leaving them as a silent omission.
    for warning in ferris_api::desk::workspace_warnings() {
        tracing::warn!("{warning}");
    }
}

#[cfg(test)]
mod tests {
    /// The catalogue the host wires at boot, as `build_router` calls it.
    fn register_both_catalogs() {
        let modules = crate::router::modules::native_modules(&ferris_core::FeatureFlags::default());
        super::register_doctype_catalog(&modules);
        super::register_workspace_catalog(&modules);
    }

    /// The workspaces this build declares, sorted: the base owners plus one per
    /// compiled vertical. Written as a build-up so the test states the rule
    /// ("a workspace exists exactly when its crate was compiled") instead of a
    /// frozen list that would need editing on every feature change.
    fn expected_workspaces() -> Vec<&'static str> {
        let mut names = vec!["build", "data", "geo", "system", "users"];
        #[cfg(feature = "email")]
        names.push("email");
        #[cfg(feature = "web")]
        names.push("website");
        #[cfg(feature = "print")]
        names.push("printing");
        #[cfg(feature = "integrations")]
        names.push("integrations");
        names.sort_unstable();
        names
    }

    /// Every first-level workspace reaches the
    /// registry, and only those.
    #[test]
    fn the_catalogue_registers_exactly_the_first_level_workspaces() {
        register_both_catalogs();
        let mut names: Vec<String> = ferris_core::workspaces::registered_workspaces()
            .iter()
            .map(|ws| ws.name.clone())
            .collect();
        names.sort();
        let expected: Vec<String> = expected_workspaces()
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(names, expected);
    }

    /// Every built-in workspace is filed under the host's single **Ferris**
    /// folder, so the grid shows one grouped icon instead of nine loose ones.
    #[test]
    fn the_framework_workspaces_are_grouped_under_the_ferris_folder() {
        register_both_catalogs();

        let folders = ferris_core::workspaces::registered_folders();
        let ferris = folders
            .iter()
            .find(|folder| folder.name == ferris_core::workspaces::FERRIS_FOLDER)
            .expect("the host declares the Ferris folder");
        assert_eq!(ferris.label, "Ferris");
        assert_eq!(ferris.icon.as_deref(), Some("Boxes"));

        let layout = ferris_api::desk::build_layout(None);
        let folder = layout
            .icons
            .iter()
            .find(|icon| icon.id == ferris_core::workspaces::FERRIS_FOLDER)
            .expect("the Ferris folder reaches the desk grid");
        assert!(folder.is_folder());
        let mut members: Vec<&str> = folder.items.iter().map(|icon| icon.id.as_str()).collect();
        members.sort_unstable();
        assert_eq!(members, expected_workspaces());
    }

    /// A declared group survives the projection to the wire, and the DocType
    /// links inside it are drawn — the DocType catalogue registered first, so
    /// the dead-link filter keeps them.
    #[test]
    fn declared_groups_reach_the_wire_with_their_links() {
        register_both_catalogs();
        let layout = ferris_api::desk::build_layout(None);
        let build = layout
            .workspaces
            .iter()
            .find(|ws| ws.name == "build")
            .expect("the Build workspace is declared");
        let customization = build
            .items
            .iter()
            .find(|item| item.kind == "group" && item.label == "Customization")
            .expect("the Customization group reaches the wire");
        assert!(
            customization
                .children
                .iter()
                .any(|item| item.doctype.as_deref() == Some("Custom Field")),
            "the group still holds its links"
        );
    }
}
