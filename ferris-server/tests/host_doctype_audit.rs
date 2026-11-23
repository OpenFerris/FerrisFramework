//! Audit of the host's DocType declarations.
//!
//! The registry validates *app* DocTypes (non-empty name, canonical fieldtype,
//! no duplicates). The host's own catalog is compiled in and trusted, so a
//! broken reference or an empty visual section would only surface as a dead
//! form later. This test registers the compiled catalog and checks the
//! invariants the Desk relies on.

use std::collections::{BTreeMap, BTreeSet};

use ferris_schema::contract::meta::{CollectionView, DocView, NamingType};
use ferris_schema::fieldtype;
use ferris_schema::registry::{DoctypeMeta, FieldMeta};
use ferris_schema::{get_doctype, list_doctypes_metas};

const ENGINE_FIELDS: &[&str] = &[
    "name",
    "owner",
    "creation",
    "modified",
    "modified_by",
    "docstatus",
    "idx",
    "parent",
    "parenttype",
    "parentfield",
    "amended_from",
    "workflow_state",
];

#[test]
fn host_doctypes_are_well_formed() {
    ferris_server::catalog::register_compiled_doctype_catalog();

    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut metas = list_doctypes_metas();
    metas.sort_by(|a, b| a.name.cmp(&b.name));
    let registered: BTreeSet<String> = metas.iter().map(|m| m.name.clone()).collect();

    for meta in &metas {
        audit(meta, &registered, &mut errors, &mut warnings);
    }

    for warning in &warnings {
        println!("WARN  {warning}");
    }
    for error in &errors {
        println!("ERROR {error}");
    }
    println!(
        "audited {} host DocTypes: {} error(s), {} warning(s)",
        metas.len(),
        errors.len(),
        warnings.len()
    );
    assert!(
        errors.is_empty(),
        "{} host DocType invariant(s) failed:\n{}",
        errors.len(),
        errors.join("\n")
    );
}

fn audit(
    meta: &DoctypeMeta,
    registered: &BTreeSet<String>,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let dt = &meta.name;
    let mut fieldnames: BTreeSet<String> = ENGINE_FIELDS.iter().map(|s| (*s).to_string()).collect();
    for field in &meta.fields {
        fieldnames.insert(field.fieldname.clone());
    }
    let by_name: BTreeMap<&str, &FieldMeta> = meta
        .fields
        .iter()
        .map(|f| (f.fieldname.as_str(), f))
        .collect();
    let err = |errors: &mut Vec<String>, message: String| errors.push(format!("[{dt}] {message}"));
    let warn =
        |warnings: &mut Vec<String>, message: String| warnings.push(format!("[{dt}] {message}"));

    let mut seen = BTreeSet::new();
    for field in &meta.fields {
        let name = field.fieldname.trim();
        if name.is_empty() {
            err(errors, "a field has an empty name".to_string());
            continue;
        }
        if !seen.insert(name) {
            err(errors, format!("field `{name}` is declared more than once"));
        }
        if !is_identifier(name) {
            err(
                errors,
                format!("field `{name}` is not a snake_case identifier"),
            );
        }
        let Some(canonical) = fieldtype::canonical(&field.fieldtype) else {
            err(
                errors,
                format!("`{name}` has unknown fieldtype `{}`", field.fieldtype),
            );
            continue;
        };
        if canonical != field.fieldtype {
            err(
                errors,
                format!(
                    "`{name}` uses `{}`; canonical is `{canonical}`",
                    field.fieldtype
                ),
            );
        }
        if fieldtype::is_layout(&field.fieldtype) {
            if field.reqd
                || field.read_only
                || field.unique
                || field.in_list_view
                || field.in_standard_filter
                || field.in_global_search
                || field.default.is_some()
                || field.permlevel != 0
            {
                err(errors, format!("layout break `{name}` carries value flags"));
            }
            if matches!(field.fieldtype.as_str(), "Tab Break" | "Section Break")
                && field.label.as_deref().is_none_or(str::is_empty)
            {
                warn(
                    warnings,
                    format!("{} `{name}` has no label", field.fieldtype),
                );
            }
            continue;
        }

        if field.label.as_deref().is_none_or(str::is_empty) {
            warn(warnings, format!("`{name}` has no label"));
        }
        if field.reqd && field.hidden && !field.read_only {
            err(
                errors,
                format!("`{name}` is required and hidden without read_only"),
            );
        }
        if field.hidden && field.in_list_view {
            err(errors, format!("`{name}` is hidden but in the list view"));
        }
        if field.hidden && field.in_standard_filter {
            err(
                errors,
                format!("`{name}` is hidden but offered as a standard filter"),
            );
        }
        if field.r#virtual && field.reqd {
            err(errors, format!("`{name}` is virtual and required"));
        }

        match field.fieldtype.as_str() {
            "Select" => {
                let options = field.options.as_deref().unwrap_or_default();
                if options.trim().is_empty() {
                    err(errors, format!("Select `{name}` has no options"));
                } else if let Some(default) = field.default.as_deref() {
                    if !default.is_empty() && !options.lines().any(|c| c.trim() == default.trim()) {
                        err(
                            errors,
                            format!("`{name}` defaults to `{default}`, which is not a choice"),
                        );
                    }
                }
            }
            "Link" | "Table" | "Table MultiSelect" => {
                let target = field.options.as_deref().unwrap_or_default().trim();
                if target.is_empty() {
                    err(
                        errors,
                        format!("{} `{name}` has no target DocType", field.fieldtype),
                    );
                } else if !registered.contains(target) {
                    err(
                        errors,
                        format!("`{name}` links to unregistered DocType `{target}`"),
                    );
                } else if matches!(field.fieldtype.as_str(), "Table" | "Table MultiSelect") {
                    if let Some(t) = get_doctype(target) {
                        if !t.is_child_table {
                            err(
                                errors,
                                format!(
                                    "`{name}` uses `{target}` as a table, but it is not a child table"
                                ),
                            );
                        }
                    }
                }
            }
            "Dynamic Link" => {
                let options = field.options.as_deref().unwrap_or_default();
                match by_name.get(options) {
                    None => err(
                        errors,
                        format!("Dynamic Link `{name}` points at missing field `{options}`"),
                    ),
                    Some(target) if target.fieldtype == "Link" => {}
                    Some(target) if target.fieldtype == "Select" => {
                        for choice in target
                            .options
                            .as_deref()
                            .unwrap_or_default()
                            .lines()
                            .map(str::trim)
                            .filter(|choice| !choice.is_empty())
                        {
                            // `URL` is the shortcut's non-doctype sentinel.
                            if choice != "URL" && !registered.contains(choice) {
                                err(
                                    errors,
                                    format!(
                                        "Dynamic Link `{name}` can select `{choice}`, which is not a registered DocType"
                                    ),
                                );
                            }
                        }
                    }
                    Some(_) => err(
                        errors,
                        format!(
                            "Dynamic Link `{name}` points at `{options}`, which is neither a Link nor a Select"
                        ),
                    ),
                }
            }
            "Image" => {
                let options = field.options.as_deref().unwrap_or_default();
                if !options.is_empty() && !by_name.contains_key(options) {
                    err(
                        errors,
                        format!("Image `{name}` points at missing field `{options}`"),
                    );
                }
            }
            _ => {}
        }

        if let Some(expression) = field.fetch_from.as_deref() {
            let mut parts = expression.split('.');
            let link = parts.next().unwrap_or_default();
            let source = parts.next().unwrap_or_default();
            let Some(link_field) = by_name.get(link) else {
                err(
                    errors,
                    format!("`{name}` fetches from missing link field `{link}`"),
                );
                continue;
            };
            if link_field.fieldtype != "Link" {
                err(
                    errors,
                    format!("`{name}` fetches through `{link}`, which is not a Link"),
                );
            } else if let Some(target) = link_field.options.as_deref().and_then(get_doctype) {
                if !target.fields.iter().any(|f| f.fieldname == source) {
                    err(
                        errors,
                        format!("`{name}` fetches `{source}`, absent on `{}`", target.name),
                    );
                }
            }
        }
    }

    let check_ref = |field: &Option<String>, label: &str, errors: &mut Vec<String>| {
        if let Some(name) = field {
            if !fieldnames.contains(name) {
                errors.push(format!("[{dt}] {label} points at missing field `{name}`"));
            }
        }
    };
    check_ref(&meta.title_field, "title_field", errors);
    check_ref(&meta.sort_field, "sort_field", errors);
    check_ref(&meta.image_field, "image_field", errors);
    check_ref(&meta.date_field, "date_field", errors);
    check_ref(&meta.color_field, "color_field", errors);
    check_ref(&meta.icon_field, "icon_field", errors);
    check_ref(&meta.kanban_group_field, "kanban_group_field", errors);
    check_ref(&meta.calendar_end_field, "calendar_end_field", errors);
    check_ref(&meta.calendar_title_field, "calendar_title_field", errors);
    check_ref(&meta.tree_parent_field, "tree_parent_field", errors);
    check_ref(&meta.tree_label_field, "tree_label_field", errors);
    check_ref(&meta.gantt_start_field, "gantt_start_field", errors);
    check_ref(&meta.gantt_end_field, "gantt_end_field", errors);
    check_ref(
        &meta.gantt_dependency_field,
        "gantt_dependency_field",
        errors,
    );
    check_ref(&meta.gantt_progress_field, "gantt_progress_field", errors);
    for field in &meta.search_fields {
        if !fieldnames.contains(field) {
            err(
                errors,
                format!("search_fields names missing field `{field}`"),
            );
        }
    }
    for field in &meta.kanban_card_fields {
        if !fieldnames.contains(field) {
            err(
                errors,
                format!("kanban_card_fields names missing field `{field}`"),
            );
        }
    }
    if let Some(order) = meta.sort_order.as_deref() {
        if !matches!(order, "asc" | "desc") {
            err(
                errors,
                format!("sort_order is `{order}`, expected `asc` or `desc`"),
            );
        }
    }
    if meta.sort_field.is_some() && meta.sort_order.is_none() {
        err(errors, "sort_field is set without sort_order".to_string());
    }
    if meta.sort_order.is_some() && meta.sort_field.is_none() {
        err(errors, "sort_order is set without sort_field".to_string());
    }
    if let Some(title) = meta.title_field.as_deref() {
        if title == "name" {
            err(
                errors,
                "title_field is `name`; the engine already uses it as the id".to_string(),
            );
        } else if let Some(field) = by_name.get(title) {
            if field.hidden {
                err(errors, format!("title_field `{title}` is hidden"));
            }
        }
    }

    let visible = !meta.hidden && !meta.is_child_table;
    let list_fields: Vec<&FieldMeta> = meta
        .fields
        .iter()
        .filter(|f| f.in_list_view && !fieldtype::is_layout(&f.fieldtype))
        .collect();
    if visible && !meta.is_single && list_fields.is_empty() {
        warn(
            warnings,
            "visible DocType has no field in the list view".to_string(),
        );
    }
    if list_fields.len() > 6 {
        warn(
            warnings,
            format!("list view declares {} columns", list_fields.len()),
        );
    }

    if meta.is_single && (meta.is_child_table || meta.is_submittable || meta.is_tree) {
        err(
            errors,
            "is_single cannot be combined with child_table, submittable or tree".to_string(),
        );
    }
    if meta.is_child_table
        && (meta.is_single || meta.is_submittable || meta.is_tree || meta.is_virtual)
    {
        err(
            errors,
            "child tables cannot be single, submittable, tree or virtual".to_string(),
        );
    }
    if meta.is_tree {
        if meta.tree_parent_field.is_none() {
            err(errors, "is_tree without tree_parent_field".to_string());
        }
        if !meta.in_tree_view {
            warn(warnings, "is_tree without in_tree_view".to_string());
        }
    }
    if meta.in_tree_view && !meta.is_tree {
        err(errors, "in_tree_view on a non-tree DocType".to_string());
    }
    if meta.in_kanban && meta.kanban_group_field.is_none() {
        let derived = meta.fields.iter().any(|f| {
            f.fieldtype == "Select" && f.options.as_deref().is_some_and(|o| o.lines().count() > 1)
        });
        if !derived {
            err(
                errors,
                "in_kanban without kanban_group_field and no multi-value Select".to_string(),
            );
        }
    }
    if meta.in_calendar && meta.date_field.is_none() {
        err(errors, "in_calendar without date_field".to_string());
    }
    if meta.in_gantt && (meta.gantt_start_field.is_none() || meta.gantt_end_field.is_none()) {
        err(
            errors,
            "in_gantt without gantt_start_field/gantt_end_field".to_string(),
        );
    }
    match meta.default_view {
        Some(CollectionView::Kanban) if meta.kanban_group_field.is_none() => err(
            errors,
            "default_view Kanban without kanban_group_field".to_string(),
        ),
        Some(CollectionView::Calendar) if meta.date_field.is_none() => err(
            errors,
            "default_view Calendar without date_field".to_string(),
        ),
        Some(CollectionView::Tree) if !meta.is_tree => err(
            errors,
            "default_view Tree on a non-tree DocType".to_string(),
        ),
        Some(CollectionView::Gantt)
            if meta.gantt_start_field.is_none() || meta.gantt_end_field.is_none() =>
        {
            err(
                errors,
                "default_view Gantt without gantt_start_field/gantt_end_field".to_string(),
            );
        }
        _ => {}
    }
    if meta.view == DocView::Dashboard {
        let bound = meta.dashboard.link_field.is_some()
            || meta.dashboard.dashboard.is_some()
            || meta.dashboard.document.is_some()
            || meta.name == "Dashboard";
        if !bound {
            err(
                errors,
                "DocView::Dashboard without a dashboard binding".to_string(),
            );
        }
        if let Some(link) = meta.dashboard.link_field.as_deref() {
            if !fieldnames.contains(link) {
                err(
                    errors,
                    format!("dashboard link_field names missing field `{link}`"),
                );
            }
        }
    }
    if meta.effective_naming_type() == NamingType::Field {
        match meta.naming_field.as_deref() {
            Some(field) if fieldnames.contains(field) => {}
            Some(field) => err(
                errors,
                format!("naming_field names missing field `{field}`"),
            ),
            None => err(errors, "naming type Field without naming_field".to_string()),
        }
    }

    audit_layout(meta, errors, warnings);

    if !meta.is_child_table && !meta.hidden && meta.permissions.is_empty() {
        err(errors, "no permission rules".to_string());
    }
    for rule in &meta.permissions {
        if rule.role.trim().is_empty() {
            err(errors, "permission rule with an empty role".to_string());
        }
    }
}

fn audit_layout(meta: &DoctypeMeta, errors: &mut Vec<String>, warnings: &mut Vec<String>) {
    let dt = &meta.name;
    let mut value_fields = 0usize;
    let mut tab_items = 0usize;
    let mut section_items = 0usize;
    let mut tab_label = String::new();
    let mut section_label = String::new();
    let mut previous_was_column = false;
    let mut last_was_break = false;

    for (index, field) in meta.fields.iter().enumerate() {
        if !fieldtype::is_layout(&field.fieldtype) {
            value_fields += 1;
            tab_items += 1;
            section_items += 1;
            previous_was_column = false;
            last_was_break = false;
            continue;
        }
        last_was_break = true;
        match field.fieldtype.as_str() {
            "Tab Break" => {
                if index > 0 && tab_items == 0 {
                    errors.push(format!("[{dt}] tab `{tab_label}` has no fields"));
                }
                if index > 0 && !section_label.is_empty() && section_items == 0 {
                    errors.push(format!("[{dt}] section `{section_label}` has no fields"));
                }
                tab_items = 0;
                section_items = 0;
                tab_label.clone_from(&field.fieldname);
                section_label.clear();
                previous_was_column = false;
            }
            "Section Break" => {
                if index > 0 && !section_label.is_empty() && section_items == 0 {
                    errors.push(format!("[{dt}] section `{section_label}` has no fields"));
                }
                section_items = 0;
                section_label.clone_from(
                    &field
                        .label
                        .clone()
                        .unwrap_or_else(|| field.fieldname.clone()),
                );
                previous_was_column = false;
            }
            "Column Break" => {
                if index == 0 {
                    warnings.push(format!("[{dt}] first field is a Column Break"));
                }
                if previous_was_column {
                    errors.push(format!(
                        "[{dt}] consecutive Column Break at `{}`",
                        field.fieldname
                    ));
                }
                previous_was_column = true;
            }
            _ => {}
        }
    }

    if last_was_break && value_fields > 0 {
        errors.push(format!("[{dt}] layout ends with a break"));
    }
    if tab_items == 0 && !tab_label.is_empty() {
        errors.push(format!("[{dt}] tab `{tab_label}` has no fields"));
    }
    if section_items == 0 && !section_label.is_empty() {
        errors.push(format!("[{dt}] section `{section_label}` has no fields"));
    }

    if !meta.is_child_table && value_fields >= 20 && !has_break(meta, "Tab Break") {
        warnings.push(format!("[{dt}] {value_fields} fields and no Tab Break"));
    }
    if value_fields >= 6 && !has_break(meta, "Section Break") {
        warnings.push(format!("[{dt}] {value_fields} fields and no Section Break"));
    }
}

fn has_break(meta: &DoctypeMeta, name: &str) -> bool {
    meta.fields.iter().any(|f| f.fieldtype == name)
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}
