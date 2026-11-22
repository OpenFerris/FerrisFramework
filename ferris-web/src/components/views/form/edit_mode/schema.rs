//! The data model of the field inspector: a field's editable properties and the
//! diff that produces property setters.
//!
//! Every property here is one the engine applies
//! (`ferris-schema/src/customization/mod.rs::apply_field_property`) plus
//! `insert_after`, which the engine reads from the setter itself to reorder the
//! effective field list. A property the engine ignores is deliberately absent:
//! offering a control that changes nothing would be a lie about the metadata.

use crate::api::DynField;

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
