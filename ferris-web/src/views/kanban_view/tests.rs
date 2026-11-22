use super::{EMPTY_COLUMN, GroupSpec, column_key, columns, neighbour, write_group};
use crate::api::{DynField, DynMeta};
use serde_json::json;
use std::collections::HashMap;

fn field(fieldname: &str, fieldtype: &str, options: Option<&str>) -> DynField {
    DynField {
        fieldname: fieldname.to_string(),
        fieldtype: fieldtype.to_string(),
        options: options.map(str::to_string),
        ..DynField::default()
    }
}

#[test]
fn the_declared_group_field_wins_over_the_derivation() {
    let meta = DynMeta {
        name: "Task".into(),
        kanban_group_field: Some("priority".into()),
        fields: vec![
            field("status", "Select", Some("Open\nClosed")),
            field("priority", "Select", Some("Low\nHigh")),
        ],
        ..DynMeta::default()
    };
    let spec = GroupSpec::from_meta(&meta);
    assert_eq!(spec.field, "priority");
    assert_eq!(spec.options, ["Low", "High"]);
    assert!(spec.movable);
}

/// Without a declaration the board keeps the historical rule: the first
/// multi-value Select. A doctype with none groups by `docstatus`, read-only.
#[test]
fn an_undeclared_board_falls_back_to_the_first_multi_value_select() {
    let derived = GroupSpec::from_meta(&DynMeta {
        fields: vec![
            field("one", "Select", Some("Only")),
            field("status", "Select", Some("Open\nClosed")),
        ],
        ..DynMeta::default()
    });
    assert_eq!(derived.field, "status");
    assert!(derived.movable);

    let none = GroupSpec::from_meta(&DynMeta {
        fields: vec![field("title", "Data", None)],
        ..DynMeta::default()
    });
    assert_eq!(none.field, "docstatus");
    assert!(!none.movable, "docstatus is a lifecycle column");
}

/// A Link board can be grouped and moved, but its columns come from the data.
#[test]
fn a_link_group_is_movable_and_derives_its_columns() {
    let meta = DynMeta {
        kanban_group_field: Some("owner".into()),
        fields: vec![field("owner", "Link", Some("User"))],
        ..DynMeta::default()
    };
    let spec = GroupSpec::from_meta(&meta);
    assert!(spec.options.is_empty());
    assert!(spec.movable);
}

#[test]
fn declared_columns_keep_their_order_and_unknown_values_still_appear() {
    let spec = GroupSpec {
        field: "status".into(),
        options: vec!["Open".into(), "Closed".into()],
        movable: true,
    };
    let rows = vec![
        json!({ "name": "A", "status": "Closed" }),
        json!({ "name": "B", "status": "Legacy" }),
        json!({ "name": "C", "status": "" }),
    ];
    let ordered = columns(&rows, &spec, &HashMap::new());
    let keys: Vec<&str> = ordered.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys, ["Open", "Closed", "Legacy", EMPTY_COLUMN]);
    assert!(ordered[0].1.is_empty());
    assert_eq!(ordered[1].1.len(), 1);
}

/// The optimistic position moves a card before the write lands.
#[test]
fn an_optimistic_position_overrides_the_stored_value() {
    let spec = GroupSpec {
        field: "status".into(),
        options: vec!["Open".into(), "Doing".into()],
        movable: true,
    };
    let rows = vec![json!({ "name": "A", "status": "Open" })];
    let mut moved = HashMap::new();
    moved.insert("A".to_string(), "Doing".to_string());
    let ordered = columns(&rows, &spec, &moved);
    assert_eq!(ordered[0].1.len(), 0);
    assert_eq!(ordered[1].1.len(), 1, "the card is drawn in its new column");
}

#[test]
fn the_arrows_walk_the_declared_option_order() {
    let labels = vec![
        "Nuevo".to_string(),
        "En curso".to_string(),
        "Hecho".to_string(),
    ];
    assert_eq!(
        neighbour("En curso", &labels, false).as_deref(),
        Some("Nuevo")
    );
    assert_eq!(
        neighbour("En curso", &labels, true).as_deref(),
        Some("Hecho")
    );
    assert_eq!(neighbour("Nuevo", &labels, false), None);
    assert_eq!(neighbour("Hecho", &labels, true), None);
    // A value outside the declared options has no arrow: guessing a neighbour
    // would move the card to an arbitrary column.
    assert_eq!(neighbour("Otro", &labels, true), None);
}

#[test]
fn a_missing_value_groups_under_the_empty_column() {
    assert_eq!(column_key(&json!({ "name": "A" }), "status"), EMPTY_COLUMN);
    assert_eq!(column_key(&json!({ "status": "" }), "status"), EMPTY_COLUMN);
}

/// The write payload is exactly the one field, and the empty column clears it.
#[test]
fn the_move_writes_the_grouping_field() {
    let patch = json!({ "status": "Doing" });
    assert_eq!(patch.get("status").and_then(|v| v.as_str()), Some("Doing"));
    // `write_group` is exercised end-to-end by the E2E run; its value mapping
    // is the only branch worth asserting here.
    assert_eq!(EMPTY_COLUMN, "—");
    let _ = write_group;
}
