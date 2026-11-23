//! The view selector of the `DocType` settings, and what each view can configure.
//!
//! A `DocType` declares which collection views it supports (`in_kanban`,
//! `in_calendar`, `in_gantt`, `in_tree_view`, `in_report_view`) and the Desk
//! renders the boards/calendars the metadata could enable. The layout editor
//! shows the same set: the tabs here are derived from those flags, never from a
//! doctype name, so a doctype that adds a view gets its tab without a code
//! change.
//!
//! **Only settings the engine actually applies are rendered.** Every property
//! below is applied by `ferris_schema::customization::apply_doctype_property`
//! and read by exactly one renderer:
//!
//! | View | Property | Read by |
//! |---|---|---|
//! | all | `default_view` | the index route redirect (`ListView`) |
//! | list | `sort_field`, `sort_order`, `search_fields` | `ListView` |
//! | kanban | `kanban_group_field`, `kanban_card_fields` | `KanbanView` |
//! | calendar | `date_field`, `calendar_end_field`, `calendar_title_field` | `CalendarView` |
//! | tree | `tree_parent_field`, `tree_label_field`, `tree_expanded_levels` | `TreeView` |
//! | gantt | `gantt_start_field`, `gantt_end_field`, `gantt_dependency_field`, `gantt_progress_field` | `GanttView` |
//!
//! A property whose value names a field is chosen from a picker filtered by the
//! fieldtype the renderer needs, and the server validates it again at save time
//! (`CustomizationStore::set_property`), so a typo cannot be stored and then
//! silently ignored (AGENTS.md §4).

use leptos::prelude::*;

use crate::api::DynMeta;
use crate::components::ui::icon::Icon;
use crate::i18n::{t, use_i18n};

use super::super::props::PropText;
use super::SettingsDraft;
use super::field_picker::{FieldOption, FieldPicker, PropTextSetting};

/// The collection view a tab configures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ViewKind {
    List,
    Kanban,
    Calendar,
    Tree,
    Gantt,
    Report,
}

impl ViewKind {
    /// The tab's value: stable, lowercase and independent of the label.
    pub(super) const fn value(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Kanban => "kanban",
            Self::Calendar => "calendar",
            Self::Tree => "tree",
            Self::Gantt => "gantt",
            Self::Report => "report",
        }
    }

    /// The tab label, translated for the current catalogue.
    pub(super) fn label(self, i18n: &(impl crate::i18n::Translate + Sync)) -> String {
        let key = match self {
            Self::List => "components.form.view.list",
            Self::Kanban => "components.form.view.kanban",
            Self::Calendar => "components.form.view.calendar",
            Self::Tree => "components.form.view.tree",
            Self::Gantt => "components.form.view.gantt",
            Self::Report => "components.form.view.report",
        };
        i18n.t(key)
    }

    pub(super) const fn icon(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Kanban => "kanban",
            Self::Calendar => "calendar",
            Self::Tree => "folder-tree",
            Self::Gantt => "chart-gantt",
            Self::Report => "file-text",
        }
    }
}

/// The views `meta` declares, in a fixed reading order.
///
/// `List` is always first and always present: it is the collection index of
/// every doctype (`/desk/:module/:doctype`) and where the ordering and search
/// settings apply, whether or not the doctype opts into `in_list_view`.
pub(super) fn available_views(meta: &DynMeta) -> Vec<ViewKind> {
    let mut views = vec![ViewKind::List];
    if meta.in_kanban {
        views.push(ViewKind::Kanban);
    }
    if meta.in_calendar {
        views.push(ViewKind::Calendar);
    }
    if meta.in_tree_view {
        views.push(ViewKind::Tree);
    }
    if meta.in_gantt {
        views.push(ViewKind::Gantt);
    }
    if meta.in_report_view {
        views.push(ViewKind::Report);
    }
    views
}

// ────────────────────────────────────────────────────────────────────────────
// Field options
// ────────────────────────────────────────────────────────────────────────────

fn labelled(field: &crate::api::DynField) -> FieldOption {
    FieldOption::new(
        field.fieldname.clone(),
        field
            .label
            .clone()
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| field.fieldname.clone()),
    )
}

/// Fields the user may use as a title/label.
pub(super) fn label_fields(meta: &DynMeta) -> Vec<FieldOption> {
    meta.fields
        .iter()
        .filter(|field| {
            !matches!(
                field.fieldtype.as_str(),
                "Section Break" | "Column Break" | "Tab Break" | "Table" | "HTML"
            )
        })
        .map(labelled)
        .collect()
}

/// `Select` fields with more than one option, and `Link` fields: the two
/// groupings a kanban column can be made of.
pub(super) fn grouping_fields(meta: &DynMeta) -> Vec<FieldOption> {
    meta.fields
        .iter()
        .filter(|field| match field.fieldtype.as_str() {
            "Select" => {
                field
                    .options
                    .as_deref()
                    .unwrap_or_default()
                    .lines()
                    .filter(|option| !option.trim().is_empty())
                    .count()
                    >= 2
            }
            "Link" => true,
            _ => false,
        })
        .map(labelled)
        .collect()
}

/// `Date`/`Datetime` fields.
pub(super) fn date_fields(meta: &DynMeta) -> Vec<FieldOption> {
    meta.fields
        .iter()
        .filter(|field| matches!(field.fieldtype.as_str(), "Date" | "Datetime"))
        .map(labelled)
        .collect()
}

/// `Link` fields that point at this same doctype (the tree parent and the
/// Gantt dependency).
pub(super) fn self_link_fields(meta: &DynMeta) -> Vec<FieldOption> {
    meta.fields
        .iter()
        .filter(|field| {
            field.fieldtype == "Link"
                && field.options.as_deref().map(str::trim) == Some(meta.name.as_str())
        })
        .map(labelled)
        .collect()
}

/// Numeric fields that can carry a 0–100 progress.
pub(super) fn progress_fields(meta: &DynMeta) -> Vec<FieldOption> {
    meta.fields
        .iter()
        .filter(|field| matches!(field.fieldtype.as_str(), "Percent" | "Int" | "Float"))
        .map(labelled)
        .collect()
}

/// The per-view settings of one tab.
///
/// `SettingsDraft` is eighteen `Copy` signal handles; the bundle is passed by
/// value on purpose so every tab reads the same state without cloning it.
#[allow(clippy::large_types_passed_by_value)]
#[component]
pub(super) fn ViewSettings(
    kind: ViewKind,
    /// Read for the field pickers and the kanban summary.
    #[prop(into)]
    meta: Signal<Option<DynMeta>>,
    draft: SettingsDraft,
) -> impl IntoView {
    let i18n = use_i18n();
    let options = move |pick: fn(&DynMeta) -> Vec<FieldOption>| -> Signal<Vec<FieldOption>> {
        Signal::derive(move || meta.get().map(|meta| pick(&meta)).unwrap_or_default())
    };
    let detail = match kind {
        ViewKind::List => view! {
            <PropText
                title=t("components.form.view.sort_field")
                overridden=false
                value=draft.sort_field
                hint=t("components.form.view.sort_field_hint")
            />
            <PropText
                title=t("components.form.view.sort_order")
                overridden=false
                value=draft.sort_order
                hint=t("components.form.view.sort_order_hint")
            />
            <PropText
                title=t("components.form.view.search_fields")
                overridden=false
                value=draft.search_fields
                hint=t("components.form.view.search_fields_hint")
            />
        }
        .into_any(),
        ViewKind::Kanban => view! {
            <FieldPicker
                title=t("components.form.view.group_field")
                hint=t("components.form.view.group_field_hint")
                value=draft.kanban_group_field
                options=options(grouping_fields)
            />
            <PropTextSetting
                title=t("components.form.view.card_fields")
                hint=t("components.form.view.card_fields_hint")
                value=draft.kanban_card_fields
            />
            <p class="text-xs text-muted-foreground">
                {t("components.form.view.kanban_drag_hint")}
            </p>
        }
        .into_any(),
        ViewKind::Calendar => view! {
            <FieldPicker
                title=t("components.form.view.date_field")
                hint=t("components.form.view.date_field_hint")
                value=draft.date_field
                options=options(date_fields)
            />
            <FieldPicker
                title=t("components.form.view.end_field")
                hint=t("components.form.view.calendar_end_hint")
                value=draft.calendar_end_field
                options=options(date_fields)
            />
            <FieldPicker
                title=t("components.form.view.title_field")
                hint=t("components.form.view.calendar_title_hint")
                value=draft.calendar_title_field
                options=options(label_fields)
            />
        }
        .into_any(),
        ViewKind::Tree => view! {
            <FieldPicker
                title=t("components.form.view.parent_field")
                hint=t("components.form.view.parent_field_hint")
                value=draft.tree_parent_field
                options=options(self_link_fields)
            />
            <FieldPicker
                title=t("components.form.view.label_field")
                hint=t("components.form.view.label_field_hint")
                value=draft.tree_label_field
                options=options(label_fields)
            />
            <PropTextSetting
                title=t("components.form.view.expanded_levels")
                hint=t("components.form.view.expanded_levels_hint")
                value=draft.tree_expanded_levels
            />
        }
        .into_any(),
        ViewKind::Gantt => view! {
            <FieldPicker
                title=t("components.form.view.start_field")
                hint=t("components.form.view.start_field_hint")
                value=draft.gantt_start_field
                options=options(date_fields)
            />
            <FieldPicker
                title=t("components.form.view.end_field")
                hint=t("components.form.view.gantt_end_hint")
                value=draft.gantt_end_field
                options=options(date_fields)
            />
            <FieldPicker
                title=t("components.form.view.dependency_field")
                hint=t("components.form.view.dependency_field_hint")
                value=draft.gantt_dependency_field
                options=options(self_link_fields)
            />
            <FieldPicker
                title=t("components.form.view.progress_field")
                hint=t("components.form.view.progress_field_hint")
                value=draft.gantt_progress_field
                options=options(progress_fields)
            />
        }
        .into_any(),
        ViewKind::Report => view! {
            <Unsupported
                title=t("components.form.view.report_title")
                detail=t("components.form.view.report_detail")
            />
        }
        .into_any(),
    };

    view! {
        <section class="flex flex-col gap-3" aria-label=t("components.form.view.settings_aria")>
            <div class="flex items-center gap-2">
                <Icon name=kind.icon() class="size-4 text-muted-foreground" />
                <h4 class="text-sm font-semibold">
                    {i18n.t_with(
                        "components.form.view.heading",
                        &[("view", &kind.label(&i18n))],
                    )}
                </h4>
            </div>
            {detail}
        </section>
    }
}

/// The options of the "Vista por defecto" picker: only the views the doctype
/// declares, plus the list.
#[allow(clippy::large_types_passed_by_value)] // the draft is a bundle of `Copy` handles; see `ViewSettings`
#[component]
pub(super) fn DefaultViewPicker(
    draft: SettingsDraft,
    meta: Signal<Option<DynMeta>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let options = Signal::derive(move || {
        meta.get()
            .map(|meta| {
                available_views(&meta)
                    .into_iter()
                    .map(|view| FieldOption::new(view.value(), view.label(&i18n)))
                    .collect()
            })
            .unwrap_or_default()
    });
    view! {
        <FieldPicker
            title=t("components.form.view.default_view")
            hint=t("components.form.view.default_view_hint")
            value=draft.default_view
            options=options
        />
    }
}

/// A view that is declared but has no engine-backed setting yet.
#[component]
fn Unsupported(title: String, detail: String) -> impl IntoView {
    view! {
        <div class="flex items-start gap-2 rounded-lg border border-dashed bg-muted/40 p-3">
            <Icon name="info" class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
            <div class="flex min-w-0 flex-col gap-1">
                <p class="text-sm font-medium text-foreground">{title}</p>
                <p class="text-xs text-muted-foreground">{detail}</p>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ViewKind, available_views, date_fields, grouping_fields, label_fields, progress_fields,
        self_link_fields,
    };
    use crate::api::{DynField, DynMeta};

    fn field(fieldname: &str, fieldtype: &str, options: Option<&str>) -> DynField {
        DynField {
            fieldname: fieldname.to_string(),
            label: Some(format!("Label {fieldname}")),
            fieldtype: fieldtype.to_string(),
            options: options.map(str::to_string),
            ..DynField::default()
        }
    }

    fn meta() -> DynMeta {
        DynMeta::default()
    }

    /// The selector follows the doctype's declaration, not a hardcoded list.
    #[test]
    fn the_views_a_doctype_declares_are_the_tabs_it_gets() {
        let mut base = meta();
        assert_eq!(available_views(&base), vec![ViewKind::List]);

        base.in_calendar = true;
        base.in_kanban = true;
        assert_eq!(
            available_views(&base),
            vec![ViewKind::List, ViewKind::Kanban, ViewKind::Calendar],
            "declaration order is stable regardless of the flags' order"
        );

        base.in_tree_view = true;
        base.in_gantt = true;
        base.in_report_view = true;
        assert_eq!(
            available_views(&base),
            vec![
                ViewKind::List,
                ViewKind::Kanban,
                ViewKind::Calendar,
                ViewKind::Tree,
                ViewKind::Gantt,
                ViewKind::Report,
            ]
        );
    }

    /// The kanban picker offers only the two groupings the renderer supports,
    /// and a one-option Select is not one of them.
    #[test]
    fn the_kanban_grouping_offers_selects_with_options_and_links() {
        let mut base = meta();
        assert!(grouping_fields(&base).is_empty());
        base.fields.push(field("title", "Data", None));
        base.fields.push(field("kind", "Select", Some("Only")));
        base.fields
            .push(field("status", "Select", Some("Open\nClosed")));
        base.fields.push(field("owner", "Link", Some("User")));
        let names: Vec<String> = grouping_fields(&base)
            .into_iter()
            .map(|option| option.name)
            .collect();
        assert_eq!(names, ["status", "owner"]);
    }

    #[test]
    fn date_and_progress_pickers_filter_by_fieldtype() {
        let mut base = meta();
        for field in [
            field("due", "Date", None),
            field("start", "Datetime", None),
            field("title", "Data", None),
            field("progress", "Percent", None),
            field("count", "Int", None),
            field("ratio", "Float", None),
        ] {
            base.fields.push(field);
        }
        assert_eq!(
            date_fields(&base)
                .into_iter()
                .map(|option| option.name)
                .collect::<Vec<_>>(),
            ["due", "start"]
        );
        assert_eq!(
            progress_fields(&base)
                .into_iter()
                .map(|option| option.name)
                .collect::<Vec<_>>(),
            ["progress", "count", "ratio"]
        );
    }

    /// The parent/dependency picker only offers Links back to the same doctype.
    #[test]
    fn self_links_are_the_only_parent_candidates() {
        let mut base = meta();
        base.name = "Task".to_string();
        base.fields.push(field("parent_task", "Link", Some("Task")));
        base.fields.push(field("project", "Link", Some("Project")));
        base.fields.push(field("parent_data", "Data", None));
        let names: Vec<String> = self_link_fields(&base)
            .into_iter()
            .map(|option| option.name)
            .collect();
        assert_eq!(names, ["parent_task"]);
    }

    /// A label picker offers the fields a reader can read, never a layout
    /// break or a child table.
    #[test]
    fn label_fields_skip_layout_breaks_and_tables() {
        let mut base = meta();
        for field in [
            field("title", "Data", None),
            field("section", "Section Break", None),
            field("rows", "Table", None),
        ] {
            base.fields.push(field);
        }
        let names: Vec<String> = label_fields(&base)
            .into_iter()
            .map(|option| option.name)
            .collect();
        assert_eq!(names, ["title"]);
    }
}
