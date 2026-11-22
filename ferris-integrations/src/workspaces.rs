//! The **Integrations** Desk workspace, declared by `ferris-integrations`.
//!
//! This is a pure declaration: it builds the [`WorkspaceSpec`] and hands it to
//! the framework registry under this crate's owner id. No I/O, no dependency on
//! whether the DocTypes it links exist yet — the
//! host's `register_workspace_catalog()` is what invokes it, after the DocType
//! catalog has been registered (§14.3).
//!
//! The sidebar mirrors §6.7: the connectors that have no natural group first,
//! then the Google, Authentication and Automation groups. `Google Settings` and
//! `LDAP Settings` are Singles; the Desk routes a link to a Single to its form
//! rather than a list (§13).
#![allow(clippy::doc_markdown)]

use ferris_core::workspaces::{
    FERRIS_FOLDER, WorkspaceItemSpec, WorkspaceRegistration, WorkspaceSet, WorkspaceSpec,
};

/// Registry owner: the crate id. Re-registering under it replaces this crate's
/// previous layout, so a hot reload never leaves stale links behind.
pub const OWNER: &str = "ferris-integrations";

/// Icon key for the workspace tile on the desk grid (a lucide name).
const ICON: &str = "plug";

/// A direct link to a DocType's list (or form, when it is a Single).
#[must_use]
fn doctype_link(doctype: &str) -> WorkspaceItemSpec {
    WorkspaceItemSpec {
        label: doctype.to_string(),
        doctype: Some(doctype.to_string()),
        ..WorkspaceItemSpec::default()
    }
}

/// A collapsible group of links.
#[must_use]
fn group(label: &str, children: Vec<WorkspaceItemSpec>) -> WorkspaceItemSpec {
    WorkspaceItemSpec {
        label: label.to_string(),
        children,
        ..WorkspaceItemSpec::default()
    }
}

/// The Integrations workspace, in sidebar order.
#[must_use]
pub fn integrations_workspace() -> WorkspaceSpec {
    WorkspaceSpec {
        name: "integrations".to_string(),
        label: "Integrations".to_string(),
        module: "Integrations".to_string(),
        icon: Some(ICON.to_string()),
        folder: Some(FERRIS_FOLDER.to_string()),
        items: vec![
            doctype_link("Connected App"),
            doctype_link("Push Notification"),
            doctype_link("Webhook"),
            group(
                "Google",
                vec![
                    doctype_link("Google Contacts"),
                    doctype_link("Google Calendar"),
                    doctype_link("Google Settings"),
                ],
            ),
            group(
                "Authentication",
                vec![
                    doctype_link("Social Login Key"),
                    doctype_link("LDAP Settings"),
                    doctype_link("OAuth Client"),
                    doctype_link("OAuth Provider"),
                ],
            ),
            group(
                "Automation",
                vec![
                    doctype_link("Automation Flow"),
                    doctype_link("Automation Trigger Queue"),
                    doctype_link("Assignment Rule"),
                    doctype_link("Milestone"),
                    doctype_link("Auto Repeat"),
                    doctype_link("Auto Email Report"),
                    doctype_link("Reminder"),
                ],
            ),
        ],
    }
}

/// Register this crate's Desk workspace, returning the names it registered
/// under and any validation warnings.
///
/// Idempotent per owner (the framework replaces this owner's previous layout).
#[must_use]
pub fn register_workspaces() -> WorkspaceRegistration {
    WorkspaceSet::default()
        .workspace(integrations_workspace())
        .register(OWNER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_links_every_integration_doctype_once() {
        let ws = integrations_workspace();
        let mut linked = Vec::new();
        for item in &ws.items {
            item.collect_doctypes(&mut linked);
        }
        let metas = crate::doctypes::doctype_metas();
        let mut expected: Vec<&str> = metas.iter().map(|m| m.name.as_str()).collect();
        expected.sort_unstable();
        linked.sort_unstable();
        assert_eq!(linked, expected);
    }

    #[test]
    fn register_is_idempotent_and_attributes_to_this_owner() {
        let first = register_workspaces();
        assert_eq!(first.names, vec!["integrations"]);
        assert!(first.warnings.is_empty(), "{:?}", first.warnings);
        let second = register_workspaces();
        assert_eq!(second.names, vec!["integrations"]);
    }
}
