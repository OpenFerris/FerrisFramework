//! DocTypes declared by `ferris-files` (the File section).
//!
//! The single `File` DocType: the metadata row the storage backend records for
//! every uploaded object. The Data workspace in
//! `ferris-api` links to it, but the one declaration lives here — *who links* is
//! not *who declares* (plan §11 #8, §16.6).
//!
//! The registry lives in `ferris-schema`; this module only declares the meta and
//! registers it (§14.1).
//!
//! Field sets are Frappe-shaped but trimmed to what the declaration needs: the
//! bytes and the storage key are the store's business, the row here is the
//! addressable handle (name, URL, size, owner, attachment target).
#![allow(clippy::doc_markdown)]

use ferris_schema::{DoctypeMeta, FieldMeta, RolePermission};

/// Desk module/section every DocType here belongs to.
pub const MODULE: &str = "File";

/// A file stored by the install.
#[must_use]
pub fn file_meta() -> DoctypeMeta {
    DoctypeMeta::new("File", MODULE)
        .description("A file stored by the install, addressed by its row.")
        .title_field("file_name")
        .sort("modified", "desc")
        .allow_delete()
        .in_global_search()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("file_name")
                .label("File Name")
                .required()
                .in_list_view()
                .in_global_search(),
        )
        .field(FieldMeta::data("file_url").label("File URL").in_list_view())
        .field(
            FieldMeta::check("is_private")
                .label("Is Private")
                .default_value("1")
                .in_list_view(),
        )
        .field(
            FieldMeta::check("is_folder")
                .label("Is Folder")
                .default_value("0"),
        )
        .field(FieldMeta::int("file_size").label("File Size"))
        .field(FieldMeta::data("file_type").label("File Type"))
        .field(FieldMeta::data("content_hash").label("Content Hash"))
        .field(FieldMeta::link("uploaded_by", "User").label("Uploaded By"))
        .field(FieldMeta::link("attached_to_doctype", "DocType").label("Attached To DocType"))
        .field(FieldMeta::data("attached_to_name").label("Attached To Name"))
        .field(FieldMeta::data("attached_to_field").label("Attached To Field"))
        .field(FieldMeta::link("file_owner", "User").label("File Owner"))
        .permission(system_manager())
}

/// Full CRUD for the administrator role.
///
/// The document engine's owner rule scopes users to their own files; the role
/// grant is what lets the administrator see and manage every row.
#[must_use]
fn system_manager() -> RolePermission {
    RolePermission::new("System Manager")
        .read()
        .write()
        .create()
        .delete()
}

/// Every DocType this crate declares, in registration order.
#[must_use]
pub fn doctype_metas() -> Vec<DoctypeMeta> {
    vec![file_meta()]
}

/// Register this crate's DocTypes in the global schema registry.
///
/// Idempotent: re-registering overwrites the same keys, so it is safe on every
/// boot and after a hot reload. Tables themselves are created lazily by the
/// document engine the first time a DocType is touched.
pub fn register_doctypes() {
    for meta in doctype_metas() {
        ferris_schema::register_doctype(meta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_schema::get_doctype;

    #[test]
    fn file_registers_with_fields() {
        register_doctypes();
        for meta in doctype_metas() {
            let got = get_doctype(&meta.name).expect("registered");
            assert_eq!(got.module, MODULE, "{}", meta.name);
            assert!(!got.fields.is_empty(), "{} needs fields", meta.name);
        }
    }

    #[test]
    fn file_is_not_a_single() {
        assert!(doctype_metas().iter().all(|m| !m.is_single));
    }

    #[test]
    fn register_is_idempotent() {
        register_doctypes();
        register_doctypes();
        assert!(get_doctype("File").is_some());
    }
}
