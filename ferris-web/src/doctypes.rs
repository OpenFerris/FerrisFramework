//! DocTypes declared by `ferris-web` (the Website section).
//!
//! The CMS/public-site surface: the pages, the public forms, the two Settings
//! Singles and the Customize scripts/themes. The
//! registry lives in `ferris-schema`; this module only declares the metas and
//! registers them.
//!
//! Two of them are **Singles** (`is_single`): `Website Settings` and
//! `Portal Settings`. A Single is one fixed row named after its DocType, read
//! and written through `get_single`/`save_single`; it has no list view and
//! its row is created lazily.
//!
//! Gating: this module is compiled only under `feature = "ssr"`, the only build
//! that pulls `ferris-schema/native` and therefore the registry. The wasm
//! `hydrate` build never sees it, so the API stays out of the browser bundle.
//!
//! Naming: the plan's `Webpage` is declared as Frappe's canonical `Web Page`,
//! matching the runtime type in [`crate::website::WebPage`] and the module docs
//! there ("Mirrors Frappe's `Web Page` doctype").
#![allow(clippy::doc_markdown)]

use ferris_schema::{DoctypeMeta, FieldMeta, RolePermission};

/// Desk module/section every DocType here belongs to.
pub const MODULE: &str = "Website";

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
///
/// A Single's one row is created lazily by `get_single`, which deliberately
/// skips `Permission::Create`, so a `create` grant here would be a right
/// no code path ever checks.
#[must_use]
fn system_manager_single() -> RolePermission {
    RolePermission::new("System Manager").read().write()
}

/// A public CMS page served at `route`.
#[must_use]
pub fn web_page_meta() -> DoctypeMeta {
    DoctypeMeta::new("Web Page", MODULE)
        .description("A public CMS page served at its route.")
        .title_field("title")
        .sort("modified", "desc")
        .allow_delete()
        .in_global_search()
        .field(
            FieldMeta::data("title")
                .label("Title")
                .required()
                .in_list_view()
                .in_global_search(),
        )
        .field(
            FieldMeta::data("route")
                .label("Route")
                .required()
                .unique()
                .in_list_view(),
        )
        .field(
            FieldMeta::select("content_type", "HTML\nMarkdown")
                .label("Content Type")
                .default_value("HTML"),
        )
        .field(FieldMeta::text_editor("main_section").label("Main Section"))
        .field(
            FieldMeta::check("published")
                .label("Published")
                .default_value("0")
                .in_list_view(),
        )
        .field(FieldMeta::datetime("published_on").label("Published On"))
        .field(FieldMeta::data("template").label("Template"))
        .field(FieldMeta::small_text("meta_description").label("Meta Description"))
        .field(FieldMeta::attach_image("meta_image").label("Meta Image"))
        .field(FieldMeta::code("javascript").label("JavaScript"))
        .field(FieldMeta::code("css").label("CSS"))
        .permission(system_manager())
}

/// A public form that writes into a DocType.
#[must_use]
pub fn web_form_meta() -> DoctypeMeta {
    DoctypeMeta::new("Web Form", MODULE)
        .description("A public form that creates documents of a DocType.")
        .title_field("title")
        .sort("modified", "desc")
        .allow_delete()
        .field(
            FieldMeta::data("title")
                .label("Title")
                .required()
                .in_list_view(),
        )
        .field(
            FieldMeta::data("route")
                .label("Route")
                .required()
                .unique()
                .in_list_view(),
        )
        .field(
            FieldMeta::link("doc_type", "DocType")
                .label("DocType")
                .required()
                .in_list_view(),
        )
        .field(
            FieldMeta::check("published")
                .label("Published")
                .default_value("0")
                .in_list_view(),
        )
        .field(
            FieldMeta::check("login_required")
                .label("Login Required")
                .default_value("0"),
        )
        .field(
            FieldMeta::check("allow_multiple")
                .label("Allow Multiple")
                .default_value("0"),
        )
        .field(
            FieldMeta::check("allow_edit")
                .label("Allow Edit")
                .default_value("0"),
        )
        .field(
            FieldMeta::check("show_list")
                .label("Show List")
                .default_value("0"),
        )
        .field(FieldMeta::text("success_message").label("Success Message"))
        .field(FieldMeta::data("success_url").label("Success URL"))
        .permission(system_manager())
}

/// Single: install-wide public-site defaults.
#[must_use]
pub fn website_settings_meta() -> DoctypeMeta {
    DoctypeMeta::new("Website Settings", MODULE)
        .description("Install-wide public-site defaults.")
        .single()
        .sort("modified", "desc")
        .field(FieldMeta::data("home_page").label("Home Page"))
        .field(FieldMeta::data("title_prefix").label("Title Prefix"))
        .field(FieldMeta::small_text("brand_html").label("Brand HTML"))
        .field(FieldMeta::attach_image("favicon").label("Favicon"))
        .field(
            FieldMeta::check("enable_google_indexing")
                .label("Enable Google Indexing")
                .default_value("1"),
        )
        .field(FieldMeta::data("google_analytics_id").label("Google Analytics ID"))
        .field(FieldMeta::code("robots_txt").label("robots.txt"))
        .field(FieldMeta::code("head_html").label("Head HTML"))
        .field(FieldMeta::code("footer_html").label("Footer HTML"))
        .permission(system_manager_single())
}

/// Single: how the logged-in portal is presented.
#[must_use]
pub fn portal_settings_meta() -> DoctypeMeta {
    DoctypeMeta::new("Portal Settings", MODULE)
        .description("Presentation of the logged-in user portal.")
        .single()
        .sort("modified", "desc")
        .field(FieldMeta::link("default_role", "Role").label("Default Role"))
        .field(FieldMeta::data("home_page").label("Home Page"))
        .field(
            FieldMeta::check("hide_standard_sidebar")
                .label("Hide Standard Sidebar")
                .default_value("0"),
        )
        .field(
            FieldMeta::check("show_my_workspaces")
                .label("Show My Workspaces")
                .default_value("1"),
        )
        .permission(system_manager_single())
}

/// A custom JavaScript fragment injected into the public site.
#[must_use]
pub fn website_script_meta() -> DoctypeMeta {
    DoctypeMeta::new("Website Script", MODULE)
        .description("Custom JavaScript injected into the public site.")
        .sort("modified", "desc")
        .allow_delete()
        .field(
            FieldMeta::select("apply_to", "All Pages\nSpecific Pages")
                .label("Apply To")
                .default_value("All Pages")
                .in_list_view(),
        )
        .field(FieldMeta::data("path").label("Path").in_list_view())
        .field(
            FieldMeta::check("enabled")
                .label("Enabled")
                .default_value("1")
                .in_list_view(),
        )
        .field(FieldMeta::code("script").label("Script"))
        .permission(system_manager())
}

/// A theme used to render the public site.
#[must_use]
pub fn website_theme_meta() -> DoctypeMeta {
    DoctypeMeta::new("Website Theme", MODULE)
        .description("A theme used to render the public site.")
        .title_field("theme")
        .sort("modified", "desc")
        .allow_delete()
        .field(
            FieldMeta::data("theme")
                .label("Theme")
                .required()
                .in_list_view(),
        )
        .field(
            FieldMeta::check("custom")
                .label("Custom")
                .default_value("0")
                .in_list_view(),
        )
        .field(FieldMeta::code("theme_scss").label("Theme SCSS"))
        .permission(system_manager())
}

/// Every DocType this crate declares, in registration order.
#[must_use]
pub fn doctype_metas() -> Vec<DoctypeMeta> {
    vec![
        // Direct
        web_page_meta(),
        web_form_meta(),
        website_settings_meta(),
        portal_settings_meta(),
        // Customize
        website_script_meta(),
        website_theme_meta(),
    ]
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
    fn only_website_and_portal_settings_are_singles() {
        let singles: Vec<_> = doctype_metas()
            .into_iter()
            .filter(|m| m.is_single)
            .map(|m| m.name)
            .collect();
        assert_eq!(singles, vec!["Website Settings", "Portal Settings"]);
    }

    #[test]
    fn web_page_is_named_like_frappe() {
        let names: Vec<_> = doctype_metas().into_iter().map(|m| m.name).collect();
        assert!(names.iter().any(|n| n == "Web Page"));
        assert!(!names.iter().any(|n| n == "Webpage"));
    }

    #[test]
    fn register_is_idempotent() {
        register_doctypes();
        register_doctypes();
        assert!(get_doctype("Website Settings").is_some());
        assert!(get_doctype("Web Form").is_some());
    }
}
