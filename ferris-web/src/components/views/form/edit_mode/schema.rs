//! The data model of the field inspector: a field's editable properties and the
//! diff that produces property setters.
//!
//! Every property here is one the engine applies
//! (`ferris-schema/src/customization/mod.rs::apply_field_property`) plus
//! `insert_after`, which the engine reads from the setter itself to reorder the
//! effective field list. A property the engine ignores is deliberately absent:
//! offering a control that changes nothing would be a lie about the metadata.

use std::collections::BTreeMap;

use crate::api::DynField;

/// The control a property needs: a free-text value, a number, or a flag.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AdvancedKind {
    Text,
    Number,
    Flag,
}

/// Properties the inspector exposes beyond its first section.
///
/// Every entry is applied by `apply_field_property`; the label key lives under
/// `components.form.edit_mode.prop.<name>`.
pub(super) const ADVANCED: &[(&str, AdvancedKind)] = &[
    ("precision", AdvancedKind::Number),
    ("max_length", AdvancedKind::Number),
    ("min_value", AdvancedKind::Text),
    ("max_value", AdvancedKind::Text),
    ("bold", AdvancedKind::Flag),
    ("allow_on_submit", AdvancedKind::Flag),
    ("fetch_from", AdvancedKind::Text),
    ("fetch_if_empty", AdvancedKind::Flag),
    ("in_global_search", AdvancedKind::Flag),
    ("ignore_user_permissions", AdvancedKind::Flag),
    ("permlevel", AdvancedKind::Number),
    ("mask", AdvancedKind::Flag),
];

/// The advanced properties of `field`, as strings the controls can edit.
pub(super) fn advanced_of(field: &DynField) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut number = |key: &str, value: Option<u32>| {
        out.insert(
            key.to_string(),
            value.map(|v| v.to_string()).unwrap_or_default(),
        );
    };
    number("precision", field.precision.map(u32::from));
    number("max_length", field.max_length);
    number("columns", Some(u32::from(field.columns)));
    number("permlevel", Some(u32::from(field.permlevel)));
    number("length", field.length);
    for (key, value) in [
        ("min_value", &field.min_value),
        ("max_value", &field.max_value),
        ("width", &field.width),
        ("print_width", &field.print_width),
        ("collapsible_depends_on", &field.collapsible_depends_on),
        ("mandatory_depends_on", &field.mandatory_depends_on),
        ("read_only_depends_on", &field.read_only_depends_on),
        ("fetch_from", &field.fetch_from),
    ] {
        out.insert(key.to_string(), value.clone().unwrap_or_default());
    }
    for (key, value) in [
        ("bold", field.bold),
        ("allow_on_submit", field.allow_on_submit),
        ("read_only_on_load", field.read_only_on_load),
        ("no_copy", field.no_copy),
        ("in_global_search", field.in_global_search),
        ("in_preview", field.in_preview),
        ("in_filter", field.in_filter),
        ("search_index", field.search_index),
        ("sort_options", field.sort_options),
        ("print_hide", field.print_hide),
        ("report_hide", field.report_hide),
        ("ignore_user_permissions", field.ignore_user_permissions),
        ("ignore_xss_filter", field.ignore_xss_filter),
        ("convert_to_user_timezone", field.convert_to_user_timezone),
        (
            "remember_last_selected_value",
            field.remember_last_selected_value,
        ),
        ("hide_border", field.hide_border),
        ("collapsible", field.collapsible),
        ("non_negative", field.non_negative),
        ("mask", field.mask),
        ("fetch_if_empty", field.fetch_if_empty),
    ] {
        out.insert(key.to_string(), if value { "1" } else { "0" }.to_string());
    }
    out
}

/// A field's editable properties, seeded from its effective metadata.
///
/// The booleans are not a state machine: they are the engine's own independent
/// field-flag properties (`reqd`, `hidden`, `read_only`, …), so folding them
/// into enums would misrepresent the metadata this mirrors.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct FieldDraft {
    pub fieldtype: String,
    pub label: String,
    pub options: String,
    pub default_value: String,
    pub description: String,
    pub depends_on: String,
    pub placeholder: String,
    pub reqd: bool,
    pub hidden: bool,
    pub read_only: bool,
    pub in_list_view: bool,
    pub in_standard_filter: bool,
    pub unique: bool,
    /// The `insert_after` anchor currently in force, empty when the field keeps
    /// its declared position. Not part of [`DynField`]: the position is a
    /// property setter, applied after every other property.
    pub insert_after: String,
    /// Properties from [`ADVANCED`], kept as strings so one control component
    /// can render all of them.
    pub advanced: BTreeMap<String, String>,
}

impl FieldDraft {
    pub(super) fn from_field(field: &DynField, insert_after: String) -> Self {
        Self {
            fieldtype: field.fieldtype.clone(),
            label: field.label.clone().unwrap_or_default(),
            options: field.options.clone().unwrap_or_default(),
            default_value: field.default.clone().unwrap_or_default(),
            description: field.description.clone().unwrap_or_default(),
            depends_on: field.depends_on.clone().unwrap_or_default(),
            placeholder: field.placeholder.clone().unwrap_or_default(),
            reqd: field.reqd,
            hidden: field.hidden,
            read_only: field.read_only,
            in_list_view: field.in_list_view,
            in_standard_filter: field.in_standard_filter,
            unique: field.unique,
            insert_after,
            advanced: advanced_of(field),
        }
    }
}

/// One property write the save applies.
pub(super) struct Change {
    pub property: &'static str,
    pub value: String,
    /// Empty text / `false` means "remove the override, restore the declared
    /// value" instead of writing an empty one.
    pub clear: bool,
}
