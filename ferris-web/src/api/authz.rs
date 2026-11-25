//! Role permissions: the Desk's Role Permissions Builder half.
//!
//! The server persists one `DocPerm` document per `(doctype, role, permlevel)`
//! and exposes it through four routes:
//!
//! * `GET    /api/authz/role-permissions?doctype=<doctype>` — the live rows;
//! * `POST   /api/authz/role-permissions` — upsert one row, echoing the rows;
//! * `DELETE /api/authz/role-permissions/{doctype}/{role}` — remove one role;
//! * `GET/PUT /api/doctype/permissions/{doctype}/manage` — the custom
//!   permission-type matrix.
//!
//! The types below mirror `ferris_auth::docperm::DocPermRow` and
//! `ferris_schema::contract::meta::PermissionMatrix` field-for-field, so the
//! screen and the server cannot drift. Only the wire contract lives here: the
//! HTTP calls reuse the same cookie/CSRF client as the rest of the Desk.

use ferris_web_shared::route::path_segment;
use serde::{Deserialize, Serialize};

use super::client::{api_delete_raw, api_get_raw, api_post_raw, api_put_raw};
use super::types::PermissionMatrix;

/// One persisted `DocPerm` row.
///
/// `doctype` here is the `DocType` the rule applies to (the server's `doc_type`
/// document field); the row's own document type is always `DocPerm`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocPermRow {
    pub doctype: String,
    pub role: String,
    pub permlevel: u8,
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
    pub if_owner: bool,
    pub apply_to_all_doctypes: bool,
}

impl DocPermRow {
    /// A blank row for `(doctype, role, permlevel 0)`.
    #[must_use]
    pub fn blank(doctype: &str, role: &str) -> Self {
        Self {
            doctype: doctype.to_string(),
            role: role.to_string(),
            ..Self::default()
        }
    }

    /// Whether the row grants anything at all.
    ///
    /// `apply_to_all_doctypes` is deliberately not enough on its own: a rule
    /// with no right is dead weight, and the builder removes it instead of
    /// storing an empty row.
    #[must_use]
    pub fn has_rights(&self) -> bool {
        Right::ALL.iter().any(|right| right.get(self)) || self.if_owner
    }
}

/// The 13 document rights a `DocPerm` row can grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Right {
    Read,
    Write,
    Create,
    Delete,
    Submit,
    Cancel,
    Amend,
    Print,
    Email,
    Share,
    Report,
    Import,
    Export,
}

impl Right {
    /// Every right, in the order the matrix renders its columns.
    pub const ALL: [Self; 13] = [
        Self::Read,
        Self::Write,
        Self::Create,
        Self::Delete,
        Self::Submit,
        Self::Cancel,
        Self::Amend,
        Self::Print,
        Self::Email,
        Self::Share,
        Self::Report,
        Self::Import,
        Self::Export,
    ];

    /// The wire field name, used for the column key and the ARIA label.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Create => "create",
            Self::Delete => "delete",
            Self::Submit => "submit",
            Self::Cancel => "cancel",
            Self::Amend => "amend",
            Self::Print => "print",
            Self::Email => "email",
            Self::Share => "share",
            Self::Report => "report",
            Self::Import => "import",
            Self::Export => "export",
        }
    }

    /// Read this right from a row.
    #[must_use]
    pub const fn get(self, row: &DocPermRow) -> bool {
        match self {
            Self::Read => row.read,
            Self::Write => row.write,
            Self::Create => row.create,
            Self::Delete => row.delete,
            Self::Submit => row.submit,
            Self::Cancel => row.cancel,
            Self::Amend => row.amend,
            Self::Print => row.print,
            Self::Email => row.email,
            Self::Share => row.share,
            Self::Report => row.report,
            Self::Import => row.import,
            Self::Export => row.export,
        }
    }

    /// Store this right on a row.
    pub const fn set(self, row: &mut DocPermRow, value: bool) {
        match self {
            Self::Read => row.read = value,
            Self::Write => row.write = value,
            Self::Create => row.create = value,
            Self::Delete => row.delete = value,
            Self::Submit => row.submit = value,
            Self::Cancel => row.cancel = value,
            Self::Amend => row.amend = value,
            Self::Print => row.print = value,
            Self::Email => row.email = value,
            Self::Share => row.share = value,
            Self::Report => row.report = value,
            Self::Import => row.import = value,
            Self::Export => row.export = value,
        }
    }
}

/// Every `DocPerm` row of one `DocType`.
///
/// # Errors
///
/// Returns the server's message when the session is not an administrator, or
/// when the request or its JSON cannot be read.
pub async fn list_role_permissions(doctype: &str) -> Result<Vec<DocPermRow>, String> {
    let path = format!(
        "/api/authz/role-permissions?doctype={}",
        path_segment(doctype)
    );
    let text = api_get_raw(&path).await?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

/// Upsert one row. The server answers with the `DocType`'s live rows.
///
/// # Errors
///
/// Returns the server's validation/permission message, or a transport or JSON
/// error.
pub async fn upsert_role_permission(row: &DocPermRow) -> Result<Vec<DocPermRow>, String> {
    let payload = serde_json::to_string(row).map_err(|error| error.to_string())?;
    let text = api_post_raw("/api/authz/role-permissions", &payload).await?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

/// Remove every stored rule of `role` for `doctype`, across permission levels.
///
/// # Errors
///
/// Returns the server's permission message or a transport error.
pub async fn delete_role_permission(doctype: &str, role: &str) -> Result<(), String> {
    let path = format!(
        "/api/authz/role-permissions/{}/{}",
        path_segment(doctype),
        path_segment(role)
    );
    api_delete_raw(&path).await.map(|_| ())
}

/// The role × custom-permission-type matrix of one `DocType`.
///
/// # Errors
///
/// Returns the server's permission message or a transport or JSON error.
pub async fn permission_matrix(doctype: &str) -> Result<PermissionMatrix, String> {
    let text = api_get_raw(&format!(
        "/api/doctype/permissions/{}/manage",
        path_segment(doctype)
    ))
    .await?;
    serde_json::from_str(&text).map_err(|error| error.to_string())
}

/// Grant or revoke one custom permission type for one role.
///
/// # Errors
///
/// Returns the server's permission/validation message or a transport error.
pub async fn set_custom_permission(
    doctype: &str,
    role: &str,
    perm_type: &str,
    granted: bool,
) -> Result<(), String> {
    let body = serde_json::json!({
        "role": role,
        "perm_type": perm_type,
        "granted": granted,
    })
    .to_string();
    api_put_raw(
        &format!("/api/doctype/permissions/{}/manage", path_segment(doctype)),
        &body,
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// The exact JSON the server's `DocPermRow` serializer emits. A row that
    /// decoded under a different shape would write rights the engine never
    /// reads (or drop one), so this fixture is the contract.
    const SERVER_ROW: &str = r#"{
        "doctype": "Task",
        "role": "System Manager",
        "permlevel": 0,
        "read": true,
        "write": true,
        "create": true,
        "delete": false,
        "submit": true,
        "cancel": false,
        "amend": false,
        "print": true,
        "email": false,
        "share": false,
        "report": true,
        "import": false,
        "export": true,
        "if_owner": false,
        "apply_to_all_doctypes": false
    }"#;

    #[test]
    fn doc_perm_rows_round_trip_the_server_payload() {
        let row: DocPermRow = serde_json::from_str(SERVER_ROW).expect("server row decodes");
        assert_eq!(row.doctype, "Task");
        assert_eq!(row.role, "System Manager");
        assert!(row.read && row.write && row.create && row.submit);
        assert!(!row.delete && !row.cancel && !row.amend && !row.email);
        assert!(row.report && row.export && !row.if_owner);

        let encoded = serde_json::to_value(&row).expect("row encodes");
        let expected: Value = serde_json::from_str(SERVER_ROW).unwrap();
        assert_eq!(encoded, expected);
    }

    #[test]
    fn absent_rights_decode_as_disabled() {
        let row: DocPermRow = serde_json::from_str(r#"{"doctype":"Task","role":"Guest"}"#).unwrap();
        assert_eq!(row.permlevel, 0);
        assert!(Right::ALL.iter().all(|right| !right.get(&row)));
        assert!(!row.if_owner);
        assert!(!row.apply_to_all_doctypes);
        assert!(!row.has_rights());
    }

    #[test]
    fn every_right_maps_to_one_field() {
        let mut row = DocPermRow::blank("Task", "Role");
        assert!(!row.has_rights());
        for right in Right::ALL {
            right.set(&mut row, true);
            assert!(right.get(&row), "{} must be readable", right.key());
        }
        assert!(row.has_rights());
        for right in Right::ALL {
            right.set(&mut row, false);
        }
        assert!(!row.has_rights());
    }

    /// The custom-permission matrix endpoint answers with the shared contract
    /// type the screen consumes, including the role list it renders as rows.
    #[test]
    fn permission_matrix_payload_decodes() {
        let matrix: PermissionMatrix = serde_json::from_str(
            r#"{
                "doctype": "Task",
                "roles": ["Approver", "System Manager"],
                "perm_types": ["approve"],
                "grants": [
                    {"perm_type": "approve", "role": "Approver", "granted": true}
                ],
                "editable": true
            }"#,
        )
        .unwrap();
        assert_eq!(matrix.doctype, "Task");
        assert_eq!(matrix.roles, vec!["Approver", "System Manager"]);
        assert!(matrix.grants[0].granted);
    }

    #[test]
    fn blank_rows_carry_their_key() {
        let row = DocPermRow::blank("Task", "Approver");
        assert_eq!(row.doctype, "Task");
        assert_eq!(row.role, "Approver");
        assert_eq!(row.permlevel, 0);
        assert!(json!(row).get("read").is_some());
    }
}
