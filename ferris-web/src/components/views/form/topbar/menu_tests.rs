//! The declared-action parser of the form menu.
//!
//! Pure: no signals, no owner, no store. What the menu *does* with an action is
//! the top bar's job; what the DocType's row *means* is decided here, and these
//! are the cases a regression would silently break (an action that disappears
//! from the menu, or a widget that opens the wrong app).

use super::menu::{FormMenuAction, FormMenuKind, menu_actions};
use crate::api::DynMeta;
use crate::api::meta_types::DynDocAction;

fn action(action_type: &str, action: &str, label: &str) -> DynDocAction {
    DynDocAction {
        name: String::new(),
        label: label.to_string(),
        action_type: action_type.to_string(),
        action: action.to_string(),
        group: None,
        hidden: false,
    }
}

fn meta_with(actions: Vec<DynDocAction>) -> DynMeta {
    DynMeta {
        actions,
        ..DynMeta::default()
    }
}

/// A `Route` row is a link, and a row that names no action is not offered at
/// all — an entry that can only do nothing is worse than a missing one.
#[test]
fn a_route_action_becomes_a_navigation_and_an_empty_row_is_dropped() {
    let meta = meta_with(vec![
        action("Route", "/app/ops-overview", "Ver panel"),
        action("Route", "   ", "Rota"),
    ]);
    let actions = menu_actions(Some(&meta));
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].label, "Ver panel");
    assert_eq!(
        actions[0].kind,
        FormMenuKind::Navigate("/app/ops-overview".to_string())
    );
}

/// An unlabelled row still gets a readable entry: Frappe falls back to the
/// method/route it calls, and so does the menu.
#[test]
fn an_unlabelled_action_uses_the_action_as_its_label() {
    let meta = meta_with(vec![action(
        "Server Action",
        "erpnext.selling.close_order",
        "",
    )]);
    let actions = menu_actions(Some(&meta));
    assert_eq!(actions[0].label, "erpnext.selling.close_order");
    assert_eq!(
        actions[0].kind,
        FormMenuKind::Method {
            name: "erpnext.selling.close_order".to_string()
        }
    );
}

/// Hidden rows are declared but not offered.
#[test]
fn hidden_actions_are_not_offered() {
    let mut hidden = action("Route", "/x", "Oculta");
    hidden.hidden = true;
    let meta = meta_with(vec![hidden]);
    assert!(menu_actions(Some(&meta)).is_empty());
}

/// A widget declaration may name its app (`<app>/<widget>` or `<app>:<widget>`)
/// or omit it, in which case the caller falls back to the current module.
#[test]
fn a_widget_action_carries_an_optional_app() {
    let meta = meta_with(vec![
        action("Widget", "ops/kpi-board", "KPI"),
        action("Widget", "sales:top-customers", "Clientes"),
        action("Widget", "plain-board", "Simple"),
    ]);
    let kinds: Vec<FormMenuKind> = menu_actions(Some(&meta))
        .into_iter()
        .map(|action: FormMenuAction| action.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            FormMenuKind::Widget {
                app: Some("ops".to_string()),
                widget: "kpi-board".to_string()
            },
            FormMenuKind::Widget {
                app: Some("sales".to_string()),
                widget: "top-customers".to_string()
            },
            FormMenuKind::Widget {
                app: None,
                widget: "plain-board".to_string()
            },
        ]
    );
}

/// No metadata (or no actions key in an older payload) means no custom entries;
/// the built-ins are the caller's business and stay untouched.
#[test]
fn no_metadata_means_no_custom_actions() {
    assert!(menu_actions(None).is_empty());
    assert!(menu_actions(Some(&DynMeta::default())).is_empty());
}
