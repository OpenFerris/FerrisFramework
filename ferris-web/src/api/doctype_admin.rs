//! Runtime `DocType` administration: the Desk's builder half.
//!
//! The server owns the policy and the DDL (`ferris_api::doctype_admin`); this
//! module is only the wire contract:
//!
//! * `POST /api/doctype` creates a **custom** `DocType` and answers 201 with
//!   the stored metadata;
//! * `PUT /api/doctype/{name}` edits one, provided it is `custom`;
//! * `GET /api/resource/DocType/{name}` reads the stored row, whose `custom`
//!   flag decides whether the builder may save at all. `meta_json` is the
//!   lossless projection of the full metadata, so an edit can send back every
//!   flag this screen does not expose.
//!
//! Both write routes are System Manager only. A non-admin sees the server's
//! refusal, never a silently empty form.
//!
//! The structs below mirror `ferris_schema::DoctypeMeta` and `FieldMeta` on the
//! fields the builder edits. Everything else travels in the flattened `extra`
//! map: the server defaults unknown keys on create, and an edit must not wipe
//! flags it never showed (`is_submittable`, `naming_series`, a field's
//! `read_only`, …). That is why the extra map is load-bearing, not decoration.

use std::collections::BTreeMap;

use ferris_web_shared::route::path_segment;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::client::{api_get_raw, api_post_raw, api_put_raw};

/// One field of a `DocType`, in the subset the builder edits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocField {
    pub fieldname: String,
    pub fieldtype: String,
    /// Human label. Empty is sent as `null`, which the engine treats as
    /// "derive the label from the fieldname".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<String>,
    pub reqd: bool,
    pub unique: bool,
    pub in_list_view: bool,
    /// Every other `FieldMeta` property, preserved verbatim across an edit.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One role rule of a `DocType`, mirroring `ferris_core::PermissionRule`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocPermission {
    pub role: String,
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub delete: bool,
    pub submit: bool,
    pub cancel: bool,
    pub amend: bool,
    pub print: bool,
    pub email: bool,
    pub share: bool,
    pub report: bool,
    pub import: bool,
    pub export: bool,
    pub permlevel: u8,
    pub if_owner: bool,
    pub apply_to_all_doctypes: bool,
    pub custom_permissions: Vec<String>,
    /// Future rights travel untouched, exactly like [`DocField::extra`].
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// The metadata body `POST /api/doctype` and `PUT /api/doctype/{name}` accept.
///
/// The server applies `#[serde(default)]` to its own `DoctypeMeta`, so omitting
/// a key is legal and means "default". The flattened map keeps the keys the
/// builder loaded but does not edit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocTypeMeta {
    pub name: String,
    pub module: String,
    pub fields: Vec<DocField>,
    pub is_child_table: bool,
    pub permissions: Vec<DocPermission>,
    /// Doctype-level flags the screen does not expose (`is_submittable`,
    /// `naming_series`, `title_field`, `custom`, …), preserved on edit.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A `DocType` read back from the server, with the editability flag.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadedDocType {
    /// Whether the stored row says `custom = true`. Compiled declarations are
    /// mirrored into `tabDocType` too, so this flag is the only public way to
    /// tell them apart.
    pub custom: bool,
    /// The full stored metadata. Only meaningful when [`Self::custom`].
    pub meta: DocTypeMeta,
}

/// The projection of `tabDocType` this module reads.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DocTypeRow {
    custom: bool,
    meta_json: String,
}

/// Create a runtime `DocType`.
///
/// # Errors
///
/// Returns the server's validation/permission message (duplicate or reserved
/// name, invalid field, non-admin caller) or a transport/JSON error.
pub async fn create(meta: &DocTypeMeta) -> Result<DocTypeMeta, String> {
    let payload = serde_json::to_string(meta).map_err(|error| error.to_string())?;
    let text = api_post_raw("/api/doctype", &payload).await?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

/// Edit an existing **custom** `DocType`.
///
/// # Errors
///
/// Returns the server's validation/permission message (compiled or unknown
/// `DocType`, body name different from the path, invalid field) or a
/// transport/JSON error.
pub async fn update(name: &str, meta: &DocTypeMeta) -> Result<DocTypeMeta, String> {
    let payload = serde_json::to_string(meta).map_err(|error| error.to_string())?;
    let text = api_put_raw(&format!("/api/doctype/{}", path_segment(name)), &payload).await?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

/// Read one stored `DocType` (custom or compiled) and its full metadata.
///
/// `GET /api/doctype/meta/{name}` does not expose the `custom` flag, and an
/// edit needs the complete `meta_json` to avoid dropping flags. The `DocType`
/// document carries both.
///
/// # Errors
///
/// Returns the server's message when the `DocType` does not exist, the session
/// may not read it, or its stored metadata is not valid JSON.
pub async fn load(name: &str) -> Result<LoadedDocType, String> {
    let text = api_get_raw(&format!("/api/resource/DocType/{}", path_segment(name))).await?;
    let row: DocTypeRow = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    if !row.custom {
        return Ok(LoadedDocType {
            custom: false,
            meta: DocTypeMeta::default(),
        });
    }
    if row.meta_json.trim().is_empty() {
        return Err(format!(
            "`{name}` has no stored metadata; recreate it from code or the builder"
        ));
    }
    let meta: DocTypeMeta =
        serde_json::from_str(&row.meta_json).map_err(|error| error.to_string())?;
    Ok(LoadedDocType { custom: true, meta })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field(fieldname: &str, fieldtype: &str) -> DocField {
        DocField {
            fieldname: fieldname.to_string(),
            fieldtype: fieldtype.to_string(),
            label: Some(fieldname.to_string()),
            reqd: true,
            unique: false,
            in_list_view: true,
            ..DocField::default()
        }
    }

    #[test]
    fn create_payload_uses_the_server_shape() {
        let meta = DocTypeMeta {
            name: "Invoice".to_string(),
            module: "Custom".to_string(),
            fields: vec![field("title", "Data"), field("status", "Select")],
            is_child_table: false,
            permissions: vec![DocPermission {
                role: "System Manager".to_string(),
                read: true,
                create: true,
                ..DocPermission::default()
            }],
            ..DocTypeMeta::default()
        };

        let value = serde_json::to_value(&meta).expect("the body serializes");
        assert_eq!(value["name"], "Invoice");
        assert_eq!(value["module"], "Custom");
        assert_eq!(value["is_child_table"], false);
        assert_eq!(value["fields"][0]["fieldname"], "title");
        assert_eq!(value["fields"][0]["fieldtype"], "Data");
        assert_eq!(value["fields"][0]["reqd"], true);
        // Empty optional properties are omitted, which the server defaults.
        assert!(value["fields"][0].get("options").is_none());
        assert_eq!(value["permissions"][0]["role"], "System Manager");
        assert_eq!(value["permissions"][0]["read"], true);
        assert_eq!(value["permissions"][0]["delete"], false);
    }

    #[test]
    fn options_travel_when_present() {
        let meta = DocTypeMeta {
            fields: vec![DocField {
                options: Some("Draft,Open,Closed".to_string()),
                ..field("status", "Select")
            }],
            ..DocTypeMeta::default()
        };
        let value = serde_json::to_value(&meta).unwrap();
        assert_eq!(value["fields"][0]["options"], "Draft,Open,Closed");
    }

    /// A stored `meta_json` carries the whole `DoctypeMeta`. An edit that only
    /// touched the typed subset must send the rest back byte-for-byte.
    #[test]
    fn unedited_metadata_survives_the_round_trip() {
        let stored = json!({
            "name": "Invoice",
            "module": "Custom",
            "is_submittable": true,
            "naming_series": "INV-.#####",
            "title_field": "title",
            "fields": [{
                "fieldname": "title",
                "fieldtype": "Data",
                "label": "Title",
                "reqd": false,
                "unique": false,
                "in_list_view": true,
                "read_only": true,
                "description": "kept from the stored metadata"
            }],
            "permissions": [{
                "role": "System Manager",
                "read": true,
                "write": true,
                "custom_permissions": ["approve"]
            }]
        });

        let meta: DocTypeMeta = serde_json::from_value(stored).unwrap();
        assert_eq!(meta.extra["is_submittable"], true);
        assert_eq!(meta.extra["naming_series"], "INV-.#####");
        assert_eq!(meta.fields[0].extra["read_only"], true);
        assert_eq!(
            meta.permissions[0].custom_permissions,
            vec!["approve".to_string()]
        );

        let encoded = serde_json::to_value(&meta).unwrap();
        assert_eq!(encoded["is_submittable"], true);
        assert_eq!(encoded["naming_series"], "INV-.#####");
        assert_eq!(encoded["title_field"], "title");
        assert_eq!(encoded["fields"][0]["read_only"], true);
        assert_eq!(
            encoded["fields"][0]["description"],
            "kept from the stored metadata"
        );
        assert_eq!(
            encoded["permissions"][0]["custom_permissions"],
            json!(["approve"])
        );
    }

    #[test]
    fn stored_row_without_meta_json_is_not_editable() {
        // The shape `GET /api/resource/DocType/{name}` answers with; only the
        // two keys this module reads are listed.
        let row: DocTypeRow = serde_json::from_str(
            r#"{"name": "Task", "module": "Core", "custom": false, "meta_json": ""}"#,
        )
        .unwrap();
        assert!(!row.custom);
        assert!(row.meta_json.is_empty());
    }
}
