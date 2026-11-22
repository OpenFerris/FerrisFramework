//! The **Website** Desk workspace, declared by `ferris-web`.
//!
//! A pure declaration: the module builds the [`WorkspaceSpec`] and hands it to
//! the framework registry under this crate's owner id. No I/O, and no dependency
//! on whether every DocType it links is registered — the host's
//! `register_workspace_catalog()` invokes this after the DocType catalog.
//!
//! The desktop/hydrate build has neither `ferris-core` nor the registry, so this
//! module is `#[cfg(feature = "ssr")]` like [`crate::doctypes`].
//!
//! The sidebar lists the public-site DocTypes first, then the Customize group
//! (`Website Settings` and `Portal Settings` are Singles). `Webpage` from the
//! catalogue is declared and linked as the canonical
//! `Web Page` (Frappe spelling).
#![allow(clippy::doc_markdown)]

use ferris_core::workspaces::{
    FERRIS_FOLDER, WorkspaceItemSpec as Item, WorkspaceRegistration, WorkspaceSet, WorkspaceSpec,
};

/// Registry owner: the crate id. Re-registering under it replaces this crate's
/// previous layout, so a hot reload never leaves stale links behind.
pub const OWNER: &str = "ferris-web";

/// The Website workspace, in sidebar order.
#[must_use]
pub fn website_workspace() -> WorkspaceSpec {
    WorkspaceSpec::new("website", "Website")
        .module(crate::doctypes::MODULE)
        .icon("Monitor")
        .folder(FERRIS_FOLDER)
        .item(Item::doctype("Web Page"))
        .item(Item::doctype("Web Form"))
        .item(Item::doctype("Website Settings"))
        .item(Item::doctype("Portal Settings"))
        .group(
            "Customize",
            vec![
                Item::doctype("Website Script"),
                Item::doctype("Website Theme"),
            ],
        )
}

/// Register this crate's Desk workspace, returning the names and any validation
/// warnings.
///
/// Idempotent per owner (the framework replaces this owner's previous layout).
#[must_use]
pub fn register_workspaces() -> WorkspaceRegistration {
    WorkspaceSet::default()
        .workspace(website_workspace())
        .register(OWNER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_registers_the_website_workspace() {
        let registration = register_workspaces();
        assert_eq!(registration.names, vec!["website"]);
        assert!(
            registration.warnings.is_empty(),
            "{:?}",
            registration.warnings
        );
    }

    #[test]
    fn website_links_every_doctype_this_crate_declares() {
        let ws = website_workspace();
        let mut linked = Vec::new();
        for item in &ws.items {
            item.collect_doctypes(&mut linked);
        }
        let metas = crate::doctypes::doctype_metas();
        let mut expected: Vec<&str> = metas.iter().map(|meta| meta.name.as_str()).collect();
        linked.sort_unstable();
        expected.sort_unstable();
        assert_eq!(linked, expected);
    }
}
