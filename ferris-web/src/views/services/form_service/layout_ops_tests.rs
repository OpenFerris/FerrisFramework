//! The pure half of layout undo/redo: the op → write inverses.
//!
//! Kept apart from `layout_ops.rs` so that file holds code and this one holds
//! tests, and because these run without a reactive owner: no signals, no
//! `DocStore`, only the mapping the contract depends on.

use super::layout_ops::{LayoutOp, OpWrite, drop_anchor};
use crate::api::types::{CustomFieldSpec, PropertySetterSpec};

fn created_field() -> CustomFieldSpec {
    CustomFieldSpec {
        fieldname: "priority".to_string(),
        label: Some("Prioridad".to_string()),
        fieldtype: "Select".to_string(),
        options: Some("Baja\nAlta".to_string()),
        insert_after: Some("status".to_string()),
        reqd: false,
        default: None,
        hidden: false,
        read_only: false,
    }
}

/// Creating a custom field is undone by deleting it — by name, not by spec: the
/// delete endpoint takes the fieldname and the spec was only needed to redo.
#[test]
fn a_created_field_is_undone_by_deleting_it() {
    let op = LayoutOp::FieldCreated {
        spec: created_field(),
    };
    assert_eq!(
        op.undo_write(),
        OpWrite::DeleteField {
            fieldname: "priority".to_string()
        }
    );
}

/// …and redone by putting the same spec back, which is why the whole spec is
/// stored rather than the fieldname alone.
#[test]
fn a_created_field_is_redone_with_the_same_spec() {
    let op = LayoutOp::FieldCreated {
        spec: created_field(),
    };
    let OpWrite::PutField(spec) = op.redo_write() else {
        panic!("redo of a creation must be a put");
    };
    assert_eq!(spec, created_field());
}

/// A property that already had a value is undone by writing that value back —
/// deleting it instead would drop a customization the user never removed.
#[test]
fn a_written_property_is_undone_by_restoring_the_previous_value() {
    let op = LayoutOp::PropertySet {
        fieldname: Some("status".to_string()),
        property: "label".to_string(),
        previous: Some("Estado".to_string()),
        value: "Situación".to_string(),
    };
    assert_eq!(
        op.undo_write(),
        OpWrite::PutProperty(PropertySetterSpec {
            fieldname: Some("status".to_string()),
            property: "label".to_string(),
            value: "Estado".to_string(),
        })
    );
}

/// A property that had no value is undone by deleting the setter, not by
/// writing an empty string (which is a value, and would keep the override).
#[test]
fn a_property_without_a_previous_value_is_undone_by_deleting_it() {
    let op = LayoutOp::PropertySet {
        fieldname: None,
        property: "title_field".to_string(),
        previous: None,
        value: "subject".to_string(),
    };
    assert_eq!(
        op.undo_write(),
        OpWrite::DeleteProperty {
            fieldname: None,
            property: "title_field".to_string(),
        }
    );
}

/// Redo re-applies the *new* value, including a DocType-level setter whose
/// `fieldname` is absent rather than empty.
#[test]
fn redo_reapplies_the_value_that_was_recorded() {
    let op = LayoutOp::PropertySet {
        fieldname: None,
        property: "title_field".to_string(),
        previous: Some("name".to_string()),
        value: "subject".to_string(),
    };
    assert_eq!(
        op.redo_write(),
        OpWrite::PutProperty(PropertySetterSpec {
            fieldname: None,
            property: "title_field".to_string(),
            value: "subject".to_string(),
        })
    );
}

fn names() -> Vec<String> {
    ["title", "status", "priority", "notes"]
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

/// Dropping a field onto another anchors it on the target's predecessor: the
/// engine inserts "after" that anchor, which is before the drop target.
#[test]
fn a_drop_anchors_the_dragged_field_before_the_target() {
    assert_eq!(
        drop_anchor(&names(), "notes", "status").as_deref(),
        Some("title")
    );
}

/// Dropping onto the first field is the reserved `__first__` anchor: there is
/// no predecessor to name.
#[test]
fn a_drop_on_the_first_field_uses_the_reserved_anchor() {
    assert_eq!(
        drop_anchor(&names(), "notes", "title").as_deref(),
        Some("__first__")
    );
}

/// Moving a field onto its own successor is a no-op: the anchor would be the
/// field itself, and a write would spend an undo slot on nothing.
#[test]
fn dropping_a_field_where_it_already_is_writes_nothing() {
    assert_eq!(drop_anchor(&names(), "status", "priority"), None);
    assert_eq!(drop_anchor(&names(), "status", "status"), None);
    assert_eq!(drop_anchor(&names(), "missing", "status"), None);
    assert_eq!(drop_anchor(&names(), "status", "missing"), None);
}
