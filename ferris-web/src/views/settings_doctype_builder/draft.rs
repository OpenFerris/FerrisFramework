//! Pure state and validation for the `DocType` builder.
//!
//! The screen edits a [`DocTypeDraft`]: the subset of `DoctypeMeta` a builder
//! owns plus the values it must send back untouched. Keeping this model free of
//! Leptos is what makes the interesting half — payload construction, options
//! rules, ordering — testable without a browser or a server.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use crate::api::doctype_admin::{DocField, DocPermission, DocTypeMeta};

/// The canonical fieldtypes the builder offers.
///
/// Deliberately a subset of the server's full vocabulary: these are the ones a
/// custom `DocType` needs and the server can materialize as a column. The list
/// matches the server's spellings exactly, because its allowlist is
/// case-sensitive and rejects anything else.
pub const FIELD_TYPES: [&str; 15] = [
    "Data", "Text", "Int", "Float", "Currency", "Percent", "Check", "Date", "Datetime", "Select",
    "Link", "Table", "JSON", "Password", "Secret",
];

/// The field kinds that cannot be saved without an `options` value.
#[must_use]
pub fn needs_options(fieldtype: &str) -> bool {
    matches!(fieldtype, "Select" | "Link" | "Table")
}

/// One editable field, with an opaque row id so reordering keeps its inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDraft {
    /// Stable per form session; never serialized.
    pub id: u64,
    pub fieldname: String,
    pub fieldtype: String,
    pub label: String,
    pub options: String,
    pub reqd: bool,
    pub unique: bool,
    pub in_list_view: bool,
    /// A loaded field's unedited properties, preserved on save.
    pub extra: BTreeMap<String, Value>,
}

impl FieldDraft {
    /// A blank `Data` field. `id` must be unique within its form.
    #[must_use]
    pub fn blank(id: u64) -> Self {
        Self {
            id,
            fieldname: String::new(),
            fieldtype: "Data".to_string(),
            label: String::new(),
            options: String::new(),
            reqd: false,
            unique: false,
            in_list_view: false,
            extra: BTreeMap::new(),
        }
    }

    /// Project a loaded field into an editable row.
    #[must_use]
    pub fn from_wire(id: u64, field: DocField) -> Self {
        Self {
            id,
            fieldname: field.fieldname,
            fieldtype: field.fieldtype,
            label: field.label.unwrap_or_default(),
            options: field.options.unwrap_or_default(),
            reqd: field.reqd,
            unique: field.unique,
            in_list_view: field.in_list_view,
            extra: field.extra,
        }
    }

    /// Project the row back to the wire, dropping blank optional values.
    #[must_use]
    pub fn to_wire(&self) -> DocField {
        let label = self.label.trim();
        let options = self.options.trim();
        DocField {
            fieldname: self.fieldname.trim().to_string(),
            fieldtype: self.fieldtype.clone(),
            label: (!label.is_empty()).then(|| label.to_string()),
            options: (!options.is_empty()).then(|| options.to_string()),
            reqd: self.reqd,
            unique: self.unique,
            in_list_view: self.in_list_view,
            extra: self.extra.clone(),
        }
    }
}

/// The whole form: identity, fields and what an edit must not lose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocTypeDraft {
    pub name: String,
    pub module: String,
    pub is_child_table: bool,
    pub fields: Vec<FieldDraft>,
    pub permissions: Vec<DocPermission>,
    /// Doctype-level metadata the builder does not expose.
    pub extra: BTreeMap<String, Value>,
}

/// Row ids must never collide between two loads: the field list is keyed by
/// id, and reusing `1, 2, …` for a second `DocType` would make Leptos keep the
/// previous inputs (and their values) on screen.
static NEXT_FIELD_ID: AtomicU64 = AtomicU64::new(1);

fn fresh_field_id() -> u64 {
    NEXT_FIELD_ID.fetch_add(1, Ordering::Relaxed)
}

impl Default for DocTypeDraft {
    fn default() -> Self {
        Self {
            name: String::new(),
            module: "Custom".to_string(),
            is_child_table: false,
            fields: Vec::new(),
            permissions: Vec::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl DocTypeDraft {
    /// Rebuild the form from a stored metadata body.
    #[must_use]
    pub fn from_meta(meta: DocTypeMeta) -> Self {
        let fields = meta
            .fields
            .into_iter()
            .map(|field| FieldDraft::from_wire(fresh_field_id(), field))
            .collect();
        Self {
            name: meta.name,
            module: meta.module,
            is_child_table: meta.is_child_table,
            fields,
            permissions: meta.permissions,
            extra: meta.extra,
        }
    }

    /// Append a blank field and return its row id.
    pub fn add_field(&mut self) -> u64 {
        let id = fresh_field_id();
        self.fields.push(FieldDraft::blank(id));
        id
    }

    /// Remove the row with `id`, if it still exists.
    pub fn remove_field(&mut self, id: u64) -> bool {
        let before = self.fields.len();
        self.fields.retain(|field| field.id != id);
        self.fields.len() != before
    }

    /// Move the row with `id` one place up (`delta = -1`) or down (`+1`).
    ///
    /// Returns whether the row moved: the first row cannot go up and the last
    /// cannot go down, which is exactly what the buttons' disabled state says.
    pub fn move_field(&mut self, id: u64, delta: i32) -> bool {
        let Some(index) = self.fields.iter().position(|field| field.id == id) else {
            return false;
        };
        let Some(target) = index.checked_add_signed(delta as isize) else {
            return false;
        };
        if target >= self.fields.len() {
            return false;
        }
        self.fields.swap(index, target);
        true
    }

    /// Replace the row with the same id, keeping its position.
    pub fn replace_field(&mut self, field: FieldDraft) {
        if let Some(slot) = self.fields.iter_mut().find(|row| row.id == field.id) {
            *slot = field;
        }
    }

    /// The rules the server also enforces, checked before a round trip.
    ///
    /// # Errors
    ///
    /// Returns the first problem as a sentence the user can act on.
    pub fn validate(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("Give the DocType a name.".to_string());
        }
        let table: String = name.chars().filter(|c| !c.is_whitespace()).collect();
        if !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(
                "A DocType name may contain letters, digits, spaces and underscores only."
                    .to_string(),
            );
        }
        if self.module.trim().is_empty() {
            return Err("Give the DocType a module.".to_string());
        }
        if self.fields.is_empty() {
            return Err("A DocType needs at least one field.".to_string());
        }

        let mut seen = BTreeSet::new();
        for field in &self.fields {
            let fieldname = field.fieldname.trim();
            if fieldname.is_empty() {
                return Err("Every field needs a fieldname.".to_string());
            }
            if !fieldname
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return Err(format!(
                    "Field `{fieldname}` may contain letters, digits and underscores only."
                ));
            }
            if fieldname == "data" {
                return Err("`data` is the document blob and cannot be a field name.".to_string());
            }
            if self.is_child_table
                && matches!(fieldname, "parent" | "parenttype" | "parentfield" | "idx")
            {
                return Err(format!(
                    "A child DocType cannot declare the link column `{fieldname}`."
                ));
            }
            if !seen.insert(fieldname.to_string()) {
                return Err(format!("Field `{fieldname}` is declared twice."));
            }
            if !FIELD_TYPES.contains(&field.fieldtype.as_str()) {
                return Err(format!(
                    "Field `{fieldname}` has an unsupported type `{}`.",
                    field.fieldtype
                ));
            }
            if needs_options(&field.fieldtype) && field.options.trim().is_empty() {
                return Err(format!(
                    "Field `{fieldname}` is a {} and needs options.",
                    field.fieldtype
                ));
            }
        }
        Ok(())
    }

    /// Validate and build the body the server accepts.
    ///
    /// # Errors
    ///
    /// Returns the same messages as [`Self::validate`].
    pub fn payload(&self) -> Result<DocTypeMeta, String> {
        self.validate()?;
        Ok(DocTypeMeta {
            name: self.name.trim().to_string(),
            module: self.module.trim().to_string(),
            fields: self.fields.iter().map(FieldDraft::to_wire).collect(),
            is_child_table: self.is_child_table,
            permissions: self.permissions.clone(),
            extra: self.extra.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft_with(fields: &[(&str, &str)]) -> DocTypeDraft {
        let mut draft = DocTypeDraft {
            name: "Invoice".to_string(),
            ..DocTypeDraft::default()
        };
        for (fieldname, fieldtype) in fields {
            let mut field = FieldDraft::blank(draft.add_field());
            field.fieldname = (*fieldname).to_string();
            field.fieldtype = (*fieldtype).to_string();
            if needs_options(fieldtype) {
                field.options = "Draft,Open".to_string();
            }
            draft.replace_field(field);
        }
        draft
    }

    #[test]
    fn payload_keeps_field_order_and_trims_identifiers() {
        let mut draft = draft_with(&[("title", "Data"), ("status", "Select")]);
        draft.name = "  Invoice  ".to_string();
        draft.module = " Custom ".to_string();
        draft.fields[0].fieldname = " title ".to_string();
        draft.fields[1].options = " Draft,Open ".to_string();

        let payload = draft.payload().expect("a valid draft builds a payload");
        assert_eq!(payload.name, "Invoice");
        assert_eq!(payload.module, "Custom");
        let names: Vec<&str> = payload
            .fields
            .iter()
            .map(|f| f.fieldname.as_str())
            .collect();
        assert_eq!(names, ["title", "status"]);
        assert_eq!(payload.fields[1].options.as_deref(), Some("Draft,Open"));
    }

    #[test]
    fn fields_can_be_added_removed_and_reordered() {
        let mut draft = draft_with(&[("one", "Data"), ("two", "Data"), ("three", "Data")]);
        let second = draft.fields[1].id;
        assert!(draft.move_field(second, -1));
        assert_eq!(draft.fields[0].id, second);
        assert!(!draft.move_field(second, -1), "already at the top");
        assert!(draft.move_field(second, 1));
        assert_eq!(draft.fields[1].id, second);
        assert!(draft.remove_field(second));
        assert_eq!(draft.fields.len(), 2);
        assert!(!draft.remove_field(second), "removing twice is a no-op");
    }

    #[test]
    fn a_doctype_needs_a_name_a_module_and_a_field() {
        let mut draft = DocTypeDraft::default();
        assert!(draft.validate().is_err());
        draft.name = "Invoice".to_string();
        draft.module = String::new();
        assert!(draft.validate().is_err());
        draft.module = "Custom".to_string();
        assert!(draft.validate().is_err(), "no fields yet");
        draft.add_field();
        assert!(draft.validate().is_err(), "the blank field has no name");
    }

    #[test]
    fn names_are_validated_like_the_server_does() {
        let mut draft = draft_with(&[("title", "Data")]);
        for name in ["", "   ", "Invoice; DROP TABLE users", "Invoice-2"] {
            draft.name = name.to_string();
            assert!(draft.validate().is_err(), "`{name}` must be rejected");
        }
        draft.name = "Sales Invoice".to_string();
        assert!(draft.validate().is_ok(), "spaces are allowed");
    }

    #[test]
    fn duplicate_and_blob_fieldnames_are_rejected() {
        let mut draft = draft_with(&[("title", "Data"), ("title", "Data")]);
        assert!(draft.validate().unwrap_err().contains("declared twice"));
        draft.fields[1].fieldname = "data".to_string();
        assert!(draft.validate().unwrap_err().contains("document blob"));
        draft.fields[1].fieldname = "bad name".to_string();
        assert!(draft.validate().is_err());
    }

    #[test]
    fn select_link_and_table_require_options() {
        for fieldtype in ["Select", "Link", "Table"] {
            let mut draft = draft_with(&[("target", fieldtype)]);
            draft.fields[0].options.clear();
            let message = draft.validate().unwrap_err();
            assert!(
                message.contains(fieldtype) && message.contains("options"),
                "{fieldtype}: {message}"
            );
            draft.fields[0].options = "Task".to_string();
            assert!(draft.validate().is_ok(), "{fieldtype} with options");
        }
        for fieldtype in ["Data", "Text", "JSON"] {
            let draft = draft_with(&[("value", fieldtype)]);
            assert!(draft.validate().is_ok(), "{fieldtype} needs no options");
        }
    }

    #[test]
    fn every_offered_fieldtype_is_accepted() {
        let mut draft = DocTypeDraft {
            name: "Invoice".to_string(),
            ..DocTypeDraft::default()
        };
        for (index, fieldtype) in FIELD_TYPES.iter().enumerate() {
            let id = draft.add_field();
            let mut field = FieldDraft::blank(id);
            field.fieldname = format!("field_{index}");
            field.fieldtype = (*fieldtype).to_string();
            if needs_options(fieldtype) {
                field.options = "Task".to_string();
            }
            draft.replace_field(field);
        }
        assert!(draft.validate().is_ok());
    }

    #[test]
    fn child_tables_reserve_the_link_columns() {
        let mut draft = draft_with(&[("parent", "Data")]);
        draft.is_child_table = true;
        assert!(draft.validate().unwrap_err().contains("link column"));
    }

    #[test]
    fn loaded_extras_are_sent_back_untouched() {
        let meta: DocTypeMeta = serde_json::from_value(serde_json::json!({
            "name": "Invoice",
            "module": "Custom",
            "is_submittable": true,
            "fields": [{
                "fieldname": "title",
                "fieldtype": "Data",
                "read_only": true
            }],
            "permissions": [{"role": "System Manager", "read": true, "write": true}]
        }))
        .unwrap();
        let draft = DocTypeDraft::from_meta(meta);
        let payload = draft.payload().expect("loaded metadata is valid");
        assert_eq!(payload.extra["is_submittable"], true);
        assert_eq!(payload.fields[0].extra["read_only"], true);
        assert_eq!(payload.permissions.len(), 1);
        assert_eq!(payload.permissions[0].role, "System Manager");
        assert!(payload.permissions[0].write);
    }
}
