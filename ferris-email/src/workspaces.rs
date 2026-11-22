//! The **Email** Desk workspace, declared by `ferris-email`.
//!
//! A pure declaration: the module builds the [`WorkspaceSpec`] and hands it to
//! the framework registry under this crate's owner id. No I/O, and no dependency
//! on whether every DocType it links is registered — the host's
//! `register_workspace_catalog()` invokes this after the DocType catalog
//!.
//!
//! The sidebar mirrors §6.3: the mail DocTypes first, then the Settings group
//! (`Email Settings` is a Single, `Email Group` the subscriber list).
#![allow(clippy::doc_markdown)]

use ferris_core::workspaces::{
    FERRIS_FOLDER, WorkspaceItemSpec as Item, WorkspaceRegistration, WorkspaceSet, WorkspaceSpec,
};

/// Registry owner: the crate id. Re-registering under it replaces this crate's
/// previous layout, so a hot reload never leaves stale links behind.
pub const OWNER: &str = "ferris-email";

/// The Email workspace, in sidebar order.
#[must_use]
pub fn email_workspace() -> WorkspaceSpec {
    WorkspaceSpec::new("email", "Email")
        .module(crate::doctypes::MODULE)
        .icon("Mail")
        .folder(FERRIS_FOLDER)
        .item(Item::doctype("Email Account"))
        .item(Item::doctype("Inbound Email"))
        .item(Item::doctype("Communication"))
        .item(Item::doctype("Email Domain"))
        .item(Item::doctype("Email Queue"))
        .item(Item::doctype("Email Template"))
        .group(
            "Settings",
            vec![
                Item::doctype("Email Settings"),
                Item::doctype("Email Group"),
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
        .workspace(email_workspace())
        .register(OWNER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_registers_the_email_workspace() {
        let registration = register_workspaces();
        assert_eq!(registration.names, vec!["email"]);
        assert!(
            registration.warnings.is_empty(),
            "{:?}",
            registration.warnings
        );
    }

    #[test]
    fn email_links_every_doctype_this_crate_declares() {
        let ws = email_workspace();
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
