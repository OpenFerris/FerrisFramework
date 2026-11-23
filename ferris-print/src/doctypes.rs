//! DocTypes declared by `ferris-print` (the Printing section).
//!
//! `Print Format` is the single source of truth for layouts: it references the
//! Document Type it prints and carries the engine, the template, the default
//! parameters and the page geometry. `Print Settings` is the install-wide
//! fallback those rows start from.
//!
//! The builder is not a DocType here. A visual editor is an app that reads and
//! writes these same fields through the API; keeping a `Print Format Builder`
//! table without a consumer only advertised a feature that did not exist.

#![allow(clippy::doc_markdown)]

use ferris_schema::contract::meta::NamingType;
use ferris_schema::{DoctypeMeta, FieldMeta, RolePermission};

/// Desk module/section every DocType here belongs to.
pub const MODULE: &str = "Printing";

/// Full CRUD for the administrator role.
#[must_use]
fn system_manager() -> RolePermission {
    RolePermission::new("System Manager")
        .read()
        .write()
        .create()
        .delete()
}

/// Read + write with neither create nor delete: the policy of a Single.
#[must_use]
fn system_manager_single() -> RolePermission {
    RolePermission::new("System Manager").read().write()
}

/// The print layout of one DocType.
#[must_use]
pub fn print_format_meta() -> DoctypeMeta {
    DoctypeMeta::new("Print Format", MODULE)
        .description("A print layout of one Document Type.")
        .title_field("format_name")
        .naming_type(NamingType::Field)
        .naming_field("format_name")
        .sort("modified", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("format_name")
                .label("Format Name")
                .required()
                .in_list_view(),
        )
        .field(
            FieldMeta::link("doc_type", "DocType")
                .label("Document Type")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::small_text("description").label("Description"))
        .field(
            FieldMeta::select("engine", "Jinja\nTypst")
                .label("Template Engine")
                .default_value("Jinja")
                .in_list_view(),
        )
        .field(
            FieldMeta::code("template")
                .label("Template")
                .options("Jinja")
                .required(),
        )
        .field(
            FieldMeta::data("template_dir")
                .label("Template Folder")
                .description("Relative folder under FERRIS_PRINT_TEMPLATE_DIR for Jinja includes/extends and Typst files."),
        )
        .field(
            FieldMeta::json("parameters")
                .label("Parameters")
                .default_value("{}"),
        )
        .field(
            FieldMeta::json("sample_data")
                .label("Sample Data")
                .default_value("{}"),
        )
        .field(
            FieldMeta::select("paper_size", "A3\nA4\nA5\nLetter\nLegal\nTabloid")
                .label("Paper Size")
                .default_value("A4"),
        )
        .field(
            FieldMeta::select("orientation", "Portrait\nLandscape")
                .label("Orientation")
                .default_value("Portrait"),
        )
        .field(
            FieldMeta::float("margin_top")
                .label("Top Margin (mm)")
                .default_value("20"),
        )
        .field(
            FieldMeta::float("margin_bottom")
                .label("Bottom Margin (mm)")
                .default_value("20"),
        )
        .field(
            FieldMeta::float("margin_left")
                .label("Left Margin (mm)")
                .default_value("20"),
        )
        .field(
            FieldMeta::float("margin_right")
                .label("Right Margin (mm)")
                .default_value("20"),
        )
        .field(FieldMeta::float("scale").label("Scale").default_value("1"))
        .field(
            FieldMeta::check("enabled")
                .label("Enabled")
                .default_value("1")
                .in_list_view(),
        )
        .permission(system_manager())
}

/// Single: install-wide print defaults.
#[must_use]
pub fn print_settings_meta() -> DoctypeMeta {
    DoctypeMeta::new("Print Settings", MODULE)
        .single()
        .description("Install-wide print defaults.")
        .field(
            FieldMeta::select("paper_size", "A3\nA4\nA5\nLetter\nLegal\nTabloid")
                .label("Paper Size")
                .default_value("A4"),
        )
        .field(
            FieldMeta::select("orientation", "Portrait\nLandscape")
                .label("Orientation")
                .default_value("Portrait"),
        )
        .field(FieldMeta::float("scale").label("Scale").default_value("1"))
        .permission(system_manager_single())
}

/// Every DocType this crate declares, in registration order.
#[must_use]
pub fn doctype_metas() -> Vec<DoctypeMeta> {
    vec![print_format_meta(), print_settings_meta()]
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
    fn every_doctype_registers_with_fields() {
        register_doctypes();
        for meta in doctype_metas() {
            let got = get_doctype(&meta.name).expect("registered");
            assert_eq!(got.module, MODULE, "{}", meta.name);
            assert!(!got.fields.is_empty(), "{} needs fields", meta.name);
        }
    }

    #[test]
    fn only_print_settings_is_a_single() {
        let singles: Vec<_> = doctype_metas()
            .into_iter()
            .filter(|m| m.is_single)
            .map(|m| m.name)
            .collect();
        assert_eq!(singles, vec!["Print Settings"]);
    }

    #[test]
    fn print_format_references_its_doctype_and_owns_the_layout() {
        let meta = print_format_meta();
        let fields: Vec<&str> = meta
            .fields
            .iter()
            .map(|field| field.fieldname.as_str())
            .collect();
        for expected in [
            "format_name",
            "doc_type",
            "engine",
            "template",
            "parameters",
            "sample_data",
            "paper_size",
        ] {
            assert!(
                fields.contains(&expected),
                "missing `{expected}`: {fields:?}"
            );
        }
    }

    #[test]
    fn register_is_idempotent() {
        register_doctypes();
        register_doctypes();
        assert!(get_doctype("Print Format").is_some());
        assert!(get_doctype("Print Settings").is_some());
    }
}
