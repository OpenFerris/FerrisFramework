//! The data-driven half of the form's "⋯" menu: the actions a DocType
//! declares.
//!
//! The menu used to be a fixed list of entries compiled into the Desk. A
//! DocType can describe its own document actions (`DynMeta::actions`, stored
//! `DocType Action` rows) and the list view and the form sidebar already render
//! them; this is the same contract, offered where a form user looks for it.
//!
//! The declaration is data, so parsing it is a pure function ([`menu_actions`])
//! and the execution stays in the top bar. Three kinds are understood:
//!
//! * `Route` — a link the server declared (Frappe's `action_type`);
//! * `Widget` — a plugin widget, mounted under `/plugins/<app>/widget/<widget>`;
//! * anything else — a whitelisted server method, which is how the sidebar
//!   panel already treats an unknown type. Defaulting to *method* rather than
//!   dropping the row keeps a hand-authored `DocType Action` visible instead of
//!   silently disappearing from both surfaces.
//!
//! A row with no action and no label describes nothing, so it is filtered out
//! instead of becoming a click that cannot do anything.

use crate::api::DynMeta;

/// What a [`FormMenuAction`] does when it is picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormMenuKind {
    /// Navigate to a path the DocType declared.
    Navigate(String),
    /// Call a whitelisted server method.
    Method { name: String },
    /// Open a plugin widget. `app` is `None` when the declaration did not name
    /// one; the caller then falls back to the current module, which is where a
    /// DocType's own app lives.
    Widget { app: Option<String>, widget: String },
}

/// One entry the current DocType contributes to the "⋯" menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormMenuAction {
    pub label: String,
    /// A `&'static str` because the shared menu model takes an icon *name* from
    /// a fixed set; the DocType contract carries no icon yet, so the kind picks
    /// the default.
    pub icon: &'static str,
    pub kind: FormMenuKind,
}

/// The actions `meta` declares for the form, in declaration order.
///
/// Empty for a DocType with no `DocType Action` rows, for an older payload
/// without the field, and for a form whose metadata has not landed yet — the
/// caller keeps its built-in entries either way.
#[must_use]
pub fn menu_actions(meta: Option<&DynMeta>) -> Vec<FormMenuAction> {
    let Some(meta) = meta else {
        return Vec::new();
    };
    meta.actions
        .iter()
        .filter(|action| !action.hidden)
        .filter_map(|action| {
            if action.action.trim().is_empty() {
                return None;
            }
            let label = if action.label.trim().is_empty() {
                action.action.clone()
            } else {
                action.label.clone()
            };
            let kind = if action.action_type.eq_ignore_ascii_case("route") {
                FormMenuKind::Navigate(action.action.clone())
            } else if action.action_type.eq_ignore_ascii_case("widget") {
                widget_kind(&action.action)?
            } else {
                FormMenuKind::Method {
                    name: action.action.clone(),
                }
            };
            let icon = match &kind {
                FormMenuKind::Navigate(_) => "external-link",
                FormMenuKind::Method { .. } => "play",
                FormMenuKind::Widget { .. } => "layout-dashboard",
            };
            Some(FormMenuAction { label, icon, kind })
        })
        .collect()
}

/// Parse a widget declaration: `"<app>/<widget>"`, `"<app>:<widget>"` or a bare
/// `"<widget>"`.
///
/// The app is optional because a DocType's own widgets live in its own app,
/// which the caller knows from the route; naming one is only needed when a
/// DocType points at another app's widget.
fn widget_kind(action: &str) -> Option<FormMenuKind> {
    let trimmed = action.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (app, widget) = match trimmed.split_once(['/', ':']) {
        Some((app, widget)) => (Some(app.trim().to_string()), widget.trim()),
        None => (None, trimmed),
    };
    if widget.is_empty() {
        return None;
    }
    Some(FormMenuKind::Widget {
        app: app.filter(|app| !app.is_empty()),
        widget: widget.to_string(),
    })
}
