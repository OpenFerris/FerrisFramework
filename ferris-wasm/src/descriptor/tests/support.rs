//! Fixtures shared by the descriptor projection tests.

use crate::ferris::framework::descriptor::{
    DocView as DeclaredView, Doctype, Field, FieldConfig, Index,
};

/// A doctype with every capability closed and no indexes, opening its
/// documents in the built-in form.
pub(super) fn wit_doctype(name: &str, fields: Vec<Field>) -> Doctype {
    Doctype {
        name: name.to_string(),
        is_submittable: false,
        is_single: false,
        allow_submit: false,
        allow_cancel: false,
        allow_amend: false,
        allow_delete: false,
        allow_copy: false,
        allow_print: false,
        allow_rename: false,
        in_calendar: false,
        in_kanban: false,
        in_gantt: false,
        in_tree_view: false,
        in_report_view: false,
        view: DeclaredView::Form,
        fields,
        indexes: Vec::new(),
        list_default_order: None,
    }
}

/// A bare name field: the host infers its fieldtype.
pub(super) fn bare(name: &str) -> Field {
    Field::Name(name.to_string())
}

/// A config field with a type and every other knob closed.
pub(super) fn fc_base(name: &str, fieldtype: &str) -> FieldConfig {
    FieldConfig {
        name: name.to_string(),
        fieldtype: Some(fieldtype.to_string()),
        label: None,
        options: None,
        reqd: false,
        read_only: false,
        read_only_on_load: false,
        hidden: false,
        columns: 0,
        permlevel: 0,
        placeholder: None,
        description: None,
        depends_on: None,
        min_value: None,
        max_value: None,
        precision: None,
        default: None,
        in_list_view: None,
    }
}

/// Shorthand for a config field with a type and nothing else opened.
pub(super) fn config_field(name: &str, fieldtype: &str) -> Field {
    Field::Config(fc_base(name, fieldtype))
}

pub(super) fn index(fields: &[&str], unique: bool) -> Index {
    Index {
        fields: fields.iter().map(|s| s.to_string()).collect(),
        unique,
    }
}
