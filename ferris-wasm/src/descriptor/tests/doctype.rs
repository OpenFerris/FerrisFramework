//! `DocType` declarations → registry metadata and the endpoint mirror.

use ferris_schema::contract::{DashboardView, DocView};

use super::support::{bare, config_field, fc_base, index, wit_doctype};
use crate::descriptor::doctype::{ConfigDoctype, FieldInput, doctype_meta, infer_fieldtype};
use crate::ferris::framework::descriptor::{DocView as DeclaredView, Doctype, Field, FieldConfig};

#[test]
fn doctype_meta_builds_from_describe() {
    let dt = wit_doctype(
        "Task",
        vec![bare("subject"), bare("status"), bare("due_date")],
    );
    let meta = doctype_meta(&dt, "test-app");
    assert_eq!(meta.name, "Task");
    assert_eq!(meta.module, "test-app");
    assert_eq!(meta.fields.len(), 3);
    assert_eq!(meta.fields[0].fieldtype, "Data");
    assert_eq!(meta.fields[1].fieldtype, "Select");
    assert_eq!(meta.fields[2].fieldtype, "Date");
    assert_eq!(meta.naming_series, "TASK.#####");
    // H5: destructive capabilities are NOT forced open on plugin doctypes.
    assert!(!meta.allow_delete, "delete must not be open by default");
    assert!(!meta.allow_rename, "rename must not be open by default");
    assert!(!meta.allow_copy, "copy must not be open by default");
    assert!(!meta.allow_print, "print must not be open by default");
    assert!(meta.in_create && meta.in_list_view && meta.in_dialog);
}

#[test]
fn infer_fieldtype_mapping() {
    assert_eq!(infer_fieldtype("subject"), "Data");
    assert_eq!(infer_fieldtype("Status"), "Select");
    assert_eq!(infer_fieldtype("due_date"), "Date");
    assert_eq!(infer_fieldtype("created_on"), "Data");
}

/// Drives one rich `describe()` config through the metadata builder and
/// asserts every documented knob maps.
#[test]
#[allow(clippy::too_many_lines)] // fixture plus one assertion per documented capability
fn doctype_meta_parses_rich_field_config() {
    let dt = Doctype {
        name: "Task".into(),
        is_submittable: true,
        is_single: false,
        allow_submit: true,
        allow_cancel: true,
        allow_amend: true,
        allow_delete: true,
        allow_copy: true,
        allow_print: true,
        allow_rename: true,
        in_kanban: true,
        fields: vec![
            Field::Config(FieldConfig {
                fieldtype: Some("Tab Break".into()),
                ..fc_base("General", "Tab Break")
            }),
            Field::Config(FieldConfig {
                fieldtype: Some("Section Break".into()),
                ..fc_base("Resumen", "Section Break")
            }),
            bare("subject"),
            Field::Config(FieldConfig {
                reqd: true,
                options: Some("open\nclosed\narchived".into()),
                ..fc_base("status", "Select")
            }),
            Field::Config(FieldConfig {
                options: Some("Project".into()),
                description: Some("Proyecto".into()),
                ..fc_base("project", "Link")
            }),
            Field::Config(FieldConfig {
                fieldtype: Some("Tab Break".into()),
                ..fc_base("Detalles", "Tab Break")
            }),
            Field::Config(FieldConfig {
                min_value: Some("0".into()),
                max_value: Some("100".into()),
                ..fc_base("progress", "Percent")
            }),
            Field::Config(FieldConfig {
                default: Some("1".into()),
                ..fc_base("active", "Check")
            }),
        ],
        ..wit_doctype("Task", Vec::new())
    };
    let meta = doctype_meta(&dt, "test-app");

    // Rich options / flags survive.
    assert!(meta.is_submittable);
    assert!(meta.allow_submit && meta.allow_cancel && meta.allow_amend);
    assert!(meta.allow_delete && meta.allow_copy && meta.allow_print);
    // View capabilities declared in `describe()` flow into the registry
    // metadata (the desk mode selector reads them).
    assert!(meta.in_kanban);
    assert!(!meta.in_calendar);
    assert!(meta.in_list_view);

    let status = meta
        .fields
        .iter()
        .find(|f| f.fieldname == "status")
        .unwrap();
    assert_eq!(status.fieldtype, "Select");
    assert_eq!(status.options.as_deref(), Some("open\nclosed\narchived"));
    assert!(status.reqd);

    let project = meta
        .fields
        .iter()
        .find(|f| f.fieldname == "project")
        .unwrap();
    assert_eq!(project.fieldtype, "Link");
    assert_eq!(project.options.as_deref(), Some("Project"));
    assert_eq!(project.description.as_deref(), Some("Proyecto"));

    let progress = meta
        .fields
        .iter()
        .find(|f| f.fieldname == "progress")
        .unwrap();
    assert_eq!(progress.fieldtype, "Percent");
    assert_eq!(progress.min_value.as_deref(), Some("0"));
    assert_eq!(progress.max_value.as_deref(), Some("100"));

    let active = meta
        .fields
        .iter()
        .find(|f| f.fieldname == "active")
        .unwrap();
    assert_eq!(active.fieldtype, "Check");
    assert_eq!(active.default.as_deref(), Some("1"));

    // Layout breaks are carried but hidden from the list.
    let general = meta
        .fields
        .iter()
        .find(|f| f.fieldname == "General")
        .unwrap();
    assert_eq!(general.fieldtype, "Tab Break");
    assert!(!general.in_list_view && !general.in_standard_filter);
    // A bare name declares nothing about the list, so it is not listed.
    let subject = meta
        .fields
        .iter()
        .find(|f| f.fieldname == "subject")
        .unwrap();
    assert!(!subject.in_list_view);
    // Select/Check are standard-filter candidates, Link is not.
    assert!(status.in_standard_filter);
    assert!(!project.in_standard_filter);
}

/// A plugin-declared Single keeps `is_single`, names from the doctype (no
/// naming series) and turns off the list/create affordances.
#[test]
fn single_doctype_projects_single_flags() {
    let dt = Doctype {
        is_single: true,
        fields: vec![bare("site_name")],
        ..wit_doctype("Test Settings", Vec::new())
    };
    let meta = doctype_meta(&dt, "test-app");
    assert!(meta.is_single);
    assert!(
        meta.naming_series.is_empty(),
        "a single must not name from a series: {:?}",
        meta.naming_series
    );
    assert!(!meta.in_list_view, "a single has no list view");
    assert!(!meta.in_create, "a single is not created separately");
    assert!(!meta.in_global_search);
}

/// The six `doc-view` cases project onto exactly the `(view, dashboard)`
/// pair the registry — and through it the Desk — reads, and the endpoint
/// mirror carries the same pair: one translation, two consumers, so the
/// plugin detail page cannot show a declaration the registry does not.
#[test]
fn doc_view_cases_project_to_the_registry_pair() {
    let cases = [
        (DeclaredView::Form, DocView::Form, DashboardView::default()),
        (
            DeclaredView::Custom,
            DocView::Custom,
            DashboardView::default(),
        ),
        (
            DeclaredView::DashboardNative,
            DocView::Dashboard,
            DashboardView::default(),
        ),
        (
            DeclaredView::DashboardFixed("main".to_string()),
            DocView::Dashboard,
            DashboardView {
                dashboard: Some("main".to_string()),
                ..DashboardView::default()
            },
        ),
        (
            DeclaredView::DashboardLink("panel".to_string()),
            DocView::Dashboard,
            DashboardView {
                link_field: Some("panel".to_string()),
                ..DashboardView::default()
            },
        ),
        (
            DeclaredView::DashboardDocument(
                crate::ferris::framework::descriptor::DashboardFormat {
                    widgets: None,
                    layout: Some("panel_layout".to_string()),
                    label: Some("titulo".to_string()),
                    config: Some("panel_config".to_string()),
                },
            ),
            DocView::Dashboard,
            DashboardView::document_fields(
                ferris_schema::contract::DocumentFormat::default()
                    .with_layout("panel_layout")
                    .with_label("titulo")
                    .with_config("panel_config"),
            ),
        ),
    ];
    for (declared, expect_view, expect_dashboard) in cases {
        let dt = Doctype {
            view: declared.clone(),
            ..wit_doctype("Task", Vec::new())
        };
        let meta = doctype_meta(&dt, "test-app");
        assert_eq!(
            (meta.view, &meta.dashboard),
            (expect_view, &expect_dashboard),
            "registry metadata must carry the declaration"
        );
        let mirror = ConfigDoctype::from(&dt);
        assert_eq!(
            (mirror.view, &mirror.dashboard),
            (expect_view, &expect_dashboard),
            "the endpoint mirror must carry the same declaration"
        );
    }
}

/// The list view shows what the app put in it, so `in_list_view` is opt-in:
/// a field that says nothing is not a column.
#[test]
fn list_view_membership_is_opt_in() {
    let dt = Doctype {
        fields: vec![
            bare("subject"),
            config_field("status", "Select"),
            Field::Config(FieldConfig {
                options: Some("low\nhigh".into()),
                in_list_view: Some(true),
                ..fc_base("priority", "Select")
            }),
            Field::Config(FieldConfig {
                in_list_view: Some(false),
                ..fc_base("notes", "Text")
            }),
            Field::Config(FieldConfig {
                in_list_view: Some(true),
                ..fc_base("Layout", "Section Break")
            }),
        ],
        ..wit_doctype("Task", Vec::new())
    };
    let meta = doctype_meta(&dt, "test-app");

    let listed = |fieldname: &str| {
        meta.fields
            .iter()
            .find(|f| f.fieldname == fieldname)
            .unwrap()
            .in_list_view
    };
    // Bare name, and a field that explicitly opts out: both stay out.
    assert!(!listed("subject"));
    assert!(!listed("notes"));
    // Explicit opt-in wins, and it is honoured for a layout break too: the
    // app is told what it gets rather than silently overruled.
    assert!(listed("priority"));
    assert!(listed("Layout"));
    // Saying nothing leaves the field out of the list.
    assert!(!listed("status"));
}

/// Declared indexes reach the registry metadata: they are created in the
/// database and the Desk reads them to decide which fields to show.
#[test]
fn declared_indexes_reach_the_metadata() {
    let dt = Doctype {
        fields: vec![bare("subject"), config_field("status", "Select")],
        indexes: vec![
            index(&["status"], false),
            index(&["subject", "status"], true),
        ],
        ..wit_doctype("Task", Vec::new())
    };
    let meta = doctype_meta(&dt, "test-app");

    assert_eq!(meta.indexes.len(), 2);
    assert_eq!(meta.indexes[0].fields, vec!["status".to_string()]);
    assert!(!meta.indexes[0].unique);
    assert_eq!(meta.indexes[1].fields, vec!["subject", "status"]);
    assert!(meta.indexes[1].unique);
    // Names are derived, but must be distinct: index names share one
    // namespace in some dialects.
    assert_ne!(meta.indexes[0].name, meta.indexes[1].name);
    assert!(meta.indexes.iter().all(|i| !i.name.is_empty()));
}

/// `FieldInput` (the serialization mirror) maps both `field` variants into
/// the untagged 1.x wire shape: bare string or config object.
#[test]
fn field_input_maps_both_variants() {
    let name = FieldInput::from(&Field::Name("subject".to_string()));
    let config = FieldInput::from(&config_field("status", "Select"));
    assert!(matches!(name, FieldInput::Name(n) if n == "subject"));
    match config {
        FieldInput::Config(c) => {
            assert_eq!(c.name, "status");
            assert_eq!(c.fieldtype.as_deref(), Some("Select"));
        }
        FieldInput::Name(n) => panic!("expected config for status, got name {n}"),
    }
}
