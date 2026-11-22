//! The **Printing** Desk workspace, declared by `ferris-print`.
//!
//! A pure declaration: the module builds the [`WorkspaceSpec`] and hands it to
//! the framework registry under this crate's owner id. No I/O, and no dependency
//! on whether every DocType it links is registered — the host's
//! `register_workspace_catalog()` invokes this after the DocType catalog.

#![allow(clippy::doc_markdown)]

use ferris_core::workspaces::{
    FERRIS_FOLDER, WorkspaceItemSpec as Item, WorkspaceRegistration, WorkspaceSet, WorkspaceSpec,
};

/// Registry owner: the crate id. Re-registering under it replaces this crate's
/// previous layout, so a hot reload never leaves stale links behind.
pub const OWNER: &str = "ferris-print";

/// The Printing workspace, in sidebar order.
#[must_use]
pub fn printing_workspace() -> WorkspaceSpec {
    WorkspaceSpec::new("printing", "Printing")
        .module(crate::doctypes::MODULE)
        .icon("Printer")
        .folder(FERRIS_FOLDER)
        .item(Item::doctype("Print Format"))
        .item(Item::doctype("Print Settings"))
}

/// Register this crate's Desk workspace, returning the names and any validation
/// warnings.
///
/// Idempotent per owner (the framework replaces this owner's previous layout).
#[must_use]
pub fn register_workspaces() -> WorkspaceRegistration {
    WorkspaceSet::default()
        .workspace(printing_workspace())
        .register(OWNER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_registers_the_printing_workspace() {
        let registration = register_workspaces();
        assert_eq!(registration.names, vec!["printing"]);
        assert!(
            registration.warnings.is_empty(),
            "{:?}",
            registration.warnings
        );
    }

    #[test]
    fn printing_links_every_doctype_this_crate_declares() {
        let ws = printing_workspace();
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
