//! The **System** Desk workspace, declared by the host crate.
//!
//! A pure declaration: the module builds the [`WorkspaceSpec`] and hands it to
//! the framework registry under this crate's owner id. No I/O — the host's
//! `register_workspace_catalog()` invokes this after the DocType catalog
//!.
//!
//! The host also declares the single **Ferris** grid folder. Every built-in
//! workspace names it, so the desk groups the framework surface into one folder
//! instead of scattering nine icons across the grid; app/plugin workspaces keep
//! their own folders.
//!
//! The sidebar mirrors §6.9: the system DocTypes first, then the Jobs, Logs and
//! Reports groups. `Activity Log` / `Access Log` are declared by `ferris-auth`
//! (Users) and only **linked** here — a DocType may be linked from more than one
//! workspace (the registry does not impose item uniqueness), and this is the
//! System view of the same rows. That is why this file's test hard-codes the
//! expected links instead of comparing against this crate's own metas.
#![allow(clippy::doc_markdown)]

use ferris_core::workspaces::{
    FERRIS_FOLDER, WorkspaceItemSpec as Item, WorkspaceRegistration, WorkspaceSet, WorkspaceSpec,
    ferris_folder,
};

/// Registry owner: the crate id. Re-registering under it replaces this crate's
/// previous layout, so a hot reload never leaves stale links behind.
pub const OWNER: &str = "ferris-server";

/// The System workspace, in sidebar order.
#[must_use]
pub fn system_workspace() -> WorkspaceSpec {
    WorkspaceSpec::new("system", "System")
        .module(crate::doctypes::MODULE)
        .icon("Server")
        .folder(FERRIS_FOLDER)
        .item(Item::doctype("System Settings"))
        .item(Item::doctype("Path Log"))
        .item(Item::doctype("Version"))
        .group(
            "Jobs",
            vec![
                Item::doctype("Scheduled Job Type"),
                Item::doctype("Scheduled Job Log"),
                Item::doctype("RQ Job"),
                Item::doctype("RQ Worker"),
            ],
        )
        .group(
            "Logs",
            vec![
                Item::doctype("Error Log"),
                Item::doctype("Activity Log"),
                Item::doctype("Access Log"),
                Item::doctype("API Request Log"),
                Item::doctype("View Log"),
            ],
        )
        .group("Reports", vec![])
}

/// Register this crate's Desk workspace, returning the names and any validation
/// warnings.
///
/// Idempotent per owner (the framework replaces this owner's previous layout).
#[must_use]
pub fn register_workspaces() -> WorkspaceRegistration {
    WorkspaceSet::default()
        .folder(ferris_folder())
        .workspace(system_workspace())
        .register(OWNER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_registers_the_system_workspace() {
        let registration = register_workspaces();
        assert_eq!(registration.names, vec!["system"]);
        assert!(
            registration.warnings.is_empty(),
            "{:?}",
            registration.warnings
        );
    }

    #[test]
    fn system_links_the_system_doctypes_plus_the_shared_logs() {
        let ws = system_workspace();
        let mut linked = Vec::new();
        for item in &ws.items {
            item.collect_doctypes(&mut linked);
        }
        let mut expected = vec![
            "API Request Log",
            "Access Log",
            "Activity Log",
            "Error Log",
            "Path Log",
            "RQ Job",
            "RQ Worker",
            "Scheduled Job Log",
            "Scheduled Job Type",
            "System Settings",
            "Version",
            "View Log",
        ];
        linked.sort_unstable();
        expected.sort_unstable();
        assert_eq!(linked, expected);

        // Every DocType this crate declares is linked; the two shared logs are
        // the only links backed by another crate.
        for meta in crate::doctypes::doctype_metas() {
            assert!(
                linked.contains(&meta.name.as_str()),
                "{} is not linked",
                meta.name
            );
        }
    }
}
