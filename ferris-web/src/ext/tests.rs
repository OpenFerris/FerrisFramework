use super::assets::component_tag;
use super::feed::{doctype_matches, parse_feed};
use super::mount::{header_props_json, resolve_props};
use super::*;

#[test]
fn component_tags_are_hyphenated_and_prefixed() {
    assert_eq!(component_tag("invoice-actions"), "ferris-invoice-actions");
    assert_eq!(component_tag("Sales Chart"), "ferris-sales-chart");
    assert_eq!(component_tag("Form_actions"), "ferris-form-actions");
}

/// The §9 catalog, as the host mounts it (Fase 5): the anchor names are
/// part of the wire contract — plugins bind to these exact strings — so
/// a rename here silently orphans every declaration.
#[test]
fn slot_constants_match_the_catalog_contract() {
    assert_eq!(SLOT_FORM_TOOLBAR_RIGHT, "form.toolbar.right");
    assert_eq!(SLOT_FORM_TOOLBAR_LEFT, "form.toolbar.left");
    assert_eq!(SLOT_FORM_SIDEBAR_TOP, "form.sidebar.top");
    assert_eq!(SLOT_FORM_SIDEBAR_BOTTOM, "form.sidebar.bottom");
    assert_eq!(SLOT_FORM_BEFORE_FIELDS, "form.before_fields");
    assert_eq!(SLOT_FORM_AFTER_FIELDS, "form.after_fields");
    assert_eq!(SLOT_FORM_VIEW, "form.view");
    assert_eq!(SLOT_LIST_TOOLBAR_BULK, "list.toolbar.bulk");
    assert_eq!(SLOT_LIST_BEFORE_TABLE, "list.before_table");
    assert_eq!(SLOT_LIST_ROW_ACTIONS, "list.row.actions");
    assert_eq!(SLOT_SHELL_SIDEBAR_BOTTOM, "shell.sidebar.bottom");
    assert_eq!(SLOT_SHELL_TOPBAR_RIGHT, "shell.topbar.right");
    assert_eq!(SLOT_SHELL_HEADER, "shell.header");
}

/// The header slot's `data-props` payload carries the sidebar state and
/// the whole active workspace, so an app that replaces the trigger visual
/// can render from the same contract slots/widgets use. A missing
/// workspace projects `null`, never a failure.
#[test]
fn header_props_carry_sidebar_state_and_workspace() {
    let ws = crate::app::Workspace {
        name: "projects".to_string(),
        label: "Projects".to_string(),
        module: "projects".to_string(),
        icon: Some("layers".to_string()),
        items: Vec::new(),
    };

    let with: serde_json::Value =
        serde_json::from_str(&header_props_json(Some(&ws), false)).unwrap();
    assert_eq!(with["open"], serde_json::json!(false));
    assert_eq!(with["workspace"]["name"], "projects");
    assert_eq!(with["workspace"]["label"], "Projects");
    assert_eq!(with["workspace"]["icon"], "layers");

    let without: serde_json::Value = serde_json::from_str(&header_props_json(None, true)).unwrap();
    assert_eq!(without["open"], serde_json::json!(true));
    assert!(without["workspace"].is_null());
}

#[test]
fn doctype_declarations_match_case_insensitively() {
    assert!(doctype_matches(&None, Some("Invoice")));
    assert!(doctype_matches(&None, None));
    assert!(doctype_matches(
        &Some("Invoice".to_string()),
        Some("invoice")
    ));
    assert!(!doctype_matches(&Some("Invoice".to_string()), Some("Task")));
    assert!(!doctype_matches(&Some("Invoice".to_string()), None));
}

fn ext(slot: &str, doctype: Option<&str>, component: &str) -> AppUi {
    AppUi {
        owner: component.to_string(),
        extensions: vec![UiExtension {
            slot: slot.to_string(),
            doctype: doctype.map(str::to_string),
            component: component.to_string(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// A `form.view` declaration replaces the form only for the doctype it
/// names; one without a `doctype` never matches, so a plugin cannot take
/// over every form by omission. The first declaration in feed order wins.
#[test]
fn form_view_is_doctype_scoped_and_requires_the_doctype() {
    let apps = vec![
        ext("form.view", None, "grab-all"),
        ext("form.toolbar.right", Some("Equipo"), "toolbar"),
        ext("form.view", Some("Equipo"), "equipo-editor"),
        ext("form.view", Some("Cliente"), "cliente-editor"),
    ];

    let found = form_view_in(&apps, "Equipo").expect("scoped view resolves");
    assert_eq!(found.0, "equipo-editor");
    assert_eq!(found.1.component, "equipo-editor");
    // Case-insensitive, like every other doctype-aware slot.
    assert_eq!(
        form_view_in(&apps, "equipo").map(|(_, e)| e.component),
        Some("equipo-editor".to_string())
    );
    assert_eq!(
        form_view_in(&apps, "Cliente").map(|(_, e)| e.component),
        Some("cliente-editor".to_string())
    );
    // A doctype with no declared view falls back to the built-in form.
    assert!(form_view_in(&apps, "Factura").is_none());
    // The global (doctype-less) `form.view` is ignored on every doctype.
    assert_ne!(
        form_view_in(&apps, "Factura").map(|(_, e)| e.component),
        Some("grab-all".to_string())
    );
}

/// R-7: `list.row.actions` props written as `"$field"` project the row's
/// document (JSON-typed), while plain values stay verbatim strings. A
/// missing field projects `null` instead of failing the mount.
#[test]
fn row_props_project_fields_and_keep_literals() {
    let row = serde_json::json!({
        "name": "DEV-0001",
        "device_id": "abc123",
        "battery": 87,
    });
    let declared = vec![
        ("device_id".to_string(), "$device_id".to_string()),
        ("battery".to_string(), "$battery".to_string()),
        ("missing".to_string(), "$nope".to_string()),
        ("mode".to_string(), "live".to_string()),
    ];
    let resolved = resolve_props(&declared, Some(&row));
    assert_eq!(
        resolved,
        vec![
            (
                "device_id".to_string(),
                serde_json::Value::String("abc123".to_string())
            ),
            ("battery".to_string(), serde_json::json!(87)),
            ("missing".to_string(), serde_json::Value::Null),
            (
                "mode".to_string(),
                serde_json::Value::String("live".to_string())
            ),
        ]
    );

    // No row (a form/global slot): a projection resolves to null.
    let resolved = resolve_props(&declared[..1], None);
    assert_eq!(resolved[0].1, serde_json::Value::Null);
}

/// The endpoint sends the F0 envelope, not a bare array; parsing the whole
/// payload as `Vec<AppUi>` used to fail silently and leave the desk empty
/// (no slots, no dashboards).
#[test]
fn feed_parse_unwraps_the_envelope() {
    let value = serde_json::json!({
        "revision": "deadbeef",
        "apps": [{
            "owner": "test",
            "extensions": [{
                "slot": "shell.sidebar.bottom",
                "component": "status-widget"
            }]
        }]
    });
    let (revision, apps) = parse_feed(value).expect("envelope parses");
    assert_eq!(revision, "deadbeef");
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].owner, "test");
    assert_eq!(apps[0].extensions[0].slot, "shell.sidebar.bottom");

    // The pre-F0 shape (a bare array) is rejected too, so a stale server
    // cannot silently leave the desk empty either.
    assert!(parse_feed(serde_json::json!([{ "owner": "test" }])).is_none());
}
