//! Display names for DocTypes and modules, read from the desk navigation.
//!
//! The server already translates every DocType and workspace label it puts in
//! `/api/modules` / `/api/desk-layout`, so the sidebar, the grid and the header
//! menu show one language. The breadcrumbs, page titles and pickers only have a
//! route segment (`/desk/selling/Sales Order`) and used to re-derive the name
//! from it — capitalising an identifier, which cannot be translated and spelled
//! the same DocType differently in two places.
//!
//! [`NavLabels`] closes that gap: it indexes the navigation the Desk already
//! loaded. A DocType the navigation does not list (hidden, or outside the user's
//! sidebar) falls back to the raw name, exactly as before.

use leptos::prelude::*;

use crate::app::DeskNav;
use crate::app::{Workspace, WorkspaceItem};

/// DocType and module names resolved from the navigation.
#[derive(Debug, Clone, Default)]
pub struct NavLabels {
    modules: Vec<(String, String)>,
    doctypes: Vec<(String, String)>,
}

impl NavLabels {
    /// Index the loaded workspaces. A module or DocType named twice keeps the
    /// first label, which is the order the server emitted.
    #[must_use]
    pub fn from_workspaces(workspaces: &[Workspace]) -> Self {
        let mut modules: Vec<(String, String)> = Vec::new();
        let mut doctypes: Vec<(String, String)> = Vec::new();
        for workspace in workspaces {
            push_once(
                &mut modules,
                workspace.module.clone(),
                workspace.label.clone(),
            );
            collect_items(&workspace.items, &mut doctypes);
        }
        Self { modules, doctypes }
    }

    /// The visible name of a module, or `name` when the navigation has none.
    #[must_use]
    pub fn module<'a>(&'a self, name: &'a str) -> &'a str {
        lookup(&self.modules, name).unwrap_or(name)
    }

    /// The visible name of a DocType, or `doctype` when the navigation has none.
    #[must_use]
    pub fn doctype<'a>(&'a self, doctype: &'a str) -> &'a str {
        lookup(&self.doctypes, doctype).unwrap_or(doctype)
    }
}

/// The visible name of `doctype` for the current desk, or the raw name.
///
/// Reads the navigation signal, so a component that calls it re-renders when
/// the layout finishes loading. Outside the desk shell (the login page, a
/// test) it degrades to the identifier.
#[must_use]
pub fn doctype_display(doctype: &str) -> String {
    let Some(nav) = use_context::<DeskNav>() else {
        return doctype.to_string();
    };
    nav.workspaces.with(|workspaces| {
        NavLabels::from_workspaces(workspaces)
            .doctype(doctype)
            .to_string()
    })
}

/// The visible name of `module` for the current desk, or the raw name.
#[must_use]
pub fn module_display(module: &str) -> String {
    let Some(nav) = use_context::<DeskNav>() else {
        return module.to_string();
    };
    nav.workspaces.with(|workspaces| {
        NavLabels::from_workspaces(workspaces)
            .module(module)
            .to_string()
    })
}

fn collect_items(items: &[WorkspaceItem], out: &mut Vec<(String, String)>) {
    for item in items {
        if let Some(doctype) = item.doctype.as_deref()
            && !item.label.trim().is_empty()
            && item.label != doctype
        {
            push_once(out, doctype.to_string(), item.label.clone());
        }
        collect_items(&item.children, out);
    }
}

fn push_once(out: &mut Vec<(String, String)>, key: String, value: String) {
    if !value.trim().is_empty() && !out.iter().any(|(seen, _)| *seen == key) {
        out.push((key, value));
    }
}

fn lookup<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, label)| label.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::WorkspaceItemKind;

    fn item(label: &str, doctype: Option<&str>, children: Vec<WorkspaceItem>) -> WorkspaceItem {
        WorkspaceItem {
            name: label.to_string(),
            label: label.to_string(),
            kind: WorkspaceItemKind::Doctype,
            doctype: doctype.map(str::to_string),
            icon: None,
            route: None,
            href: None,
            open_in_new_tab: false,
            children,
        }
    }

    fn workspace(name: &str, label: &str, module: &str, items: Vec<WorkspaceItem>) -> Workspace {
        Workspace {
            name: name.into(),
            label: label.into(),
            module: module.into(),
            icon: None,
            items,
        }
    }

    #[test]
    fn labels_come_from_the_navigation_and_fall_back_to_the_identifier() {
        let nav = vec![workspace(
            "selling",
            "Ventas",
            "selling",
            vec![item("Pedido de venta", Some("Sales Order"), vec![])],
        )];
        let labels = NavLabels::from_workspaces(&nav);

        assert_eq!(labels.module("selling"), "Ventas");
        assert_eq!(labels.doctype("Sales Order"), "Pedido de venta");
        assert_eq!(labels.module("buying"), "buying");
        assert_eq!(labels.doctype("Customer"), "Customer");
    }

    #[test]
    fn a_link_that_keeps_the_raw_name_is_not_a_label() {
        // `WorkspaceItemSpec::doctype` sets the label to the doctype name; the
        // untranslated form must not shadow the catalogue's own entry.
        let nav = vec![workspace(
            "users",
            "Usuarios",
            "users",
            vec![item("User", Some("User"), vec![])],
        )];
        let labels = NavLabels::from_workspaces(&nav);
        assert_eq!(labels.doctype("User"), "User");
    }

    #[test]
    fn nested_group_links_are_indexed() {
        let nav = vec![workspace(
            "tools",
            "Herramientas",
            "tools",
            vec![item(
                "Más",
                None,
                vec![item("Cliente", Some("Customer"), vec![])],
            )],
        )];
        let labels = NavLabels::from_workspaces(&nav);
        assert_eq!(labels.doctype("Customer"), "Cliente");
    }
}
