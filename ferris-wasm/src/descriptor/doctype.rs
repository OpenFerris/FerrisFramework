//! Projection of a declared `DocType` into schema metadata (the dev endpoint
//! mirror and the registry `DoctypeMeta`).

use serde::Serialize;

use ferris_schema::contract::{DashboardView, DocView, DocumentFormat};
use ferris_schema::{DoctypeMeta, FieldMeta, IndexMeta};

use crate::ferris::framework::descriptor::{
    DocView as DeclaredView, Doctype, Field, FieldConfig, Index,
};

/// Doctype section of the descriptor, as the dev endpoint serializes it.
///
/// Serialization-only mirror of the generated `descriptor::doctype`; the
/// plugin boundary is the WIT record, so this is never decoded from a guest
/// payload (the 1.x `serde_json::from_str::<ConfigDocType>` path is gone).
#[derive(Debug, Clone, Serialize)]
#[allow(clippy::struct_excessive_bools)] // each bool is an independent capability toggle the app declares in `describe()`
pub struct ConfigDoctype {
    pub name: String,
    pub is_submittable: bool,
    pub is_single: bool,
    pub allow_submit: bool,
    pub allow_cancel: bool,
    pub allow_amend: bool,
    pub allow_delete: bool,
    pub allow_copy: bool,
    pub allow_print: bool,
    pub allow_rename: bool,
    /// View capabilities: which views the doctype declaration turns on for the
    /// desk. `in_list_view` is always on for plugin doctypes (see
    /// `doctype_meta`); the rest must be declared to appear in the top-bar
    /// mode selector.
    pub in_calendar: bool,
    pub in_kanban: bool,
    pub in_gantt: bool,
    pub in_tree_view: bool,
    pub in_report_view: bool,
    pub fields: Vec<FieldInput>,
    /// Indexes over the fields above. Besides being created in the database,
    /// they decide which fields the Desk shows as list columns.
    pub indexes: Vec<ConfigIndex>,
    /// The doctype's default list ordering, `"<field> <asc|desc>"`.
    pub list_default_order: Option<String>,
    /// Which view a document of this doctype opens in, and the dashboard
    /// backing it — the same pair [`crate::descriptor::doctype_meta`] projects
    /// into the registry, carried as the Desk's wire types so the endpoint
    /// and the registry cannot read a declaration differently.
    pub view: DocView,
    pub dashboard: DashboardView,
}

impl From<&Doctype> for ConfigDoctype {
    fn from(dt: &Doctype) -> Self {
        let (view, dashboard) = project_view(dt);
        Self {
            name: dt.name.clone(),
            is_submittable: dt.is_submittable,
            is_single: dt.is_single,
            allow_submit: dt.allow_submit,
            allow_cancel: dt.allow_cancel,
            allow_amend: dt.allow_amend,
            allow_delete: dt.allow_delete,
            allow_copy: dt.allow_copy,
            allow_print: dt.allow_print,
            allow_rename: dt.allow_rename,
            in_calendar: dt.in_calendar,
            in_kanban: dt.in_kanban,
            in_gantt: dt.in_gantt,
            in_tree_view: dt.in_tree_view,
            in_report_view: dt.in_report_view,
            fields: dt.fields.iter().map(FieldInput::from).collect(),
            indexes: dt.indexes.iter().map(ConfigIndex::from).collect(),
            list_default_order: dt.list_default_order.clone(),
            view,
            dashboard,
        }
    }
}

/// A declared field is either a bare name (fieldtype inferred) or a full
/// definition with type/options/validation/render hints.
///
/// Serialized untagged so the endpoint keeps the 1.x wire shape: `"subject"`
/// for a bare name, `{...}` for a config. Mirrors the WIT `variant field`.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)] // the full config shadows the bare name by design
pub enum FieldInput {
    Name(String),
    Config(ConfigField),
}

impl From<&Field> for FieldInput {
    fn from(field: &Field) -> Self {
        match field {
            Field::Name(name) => Self::Name(name.clone()),
            Field::Config(config) => Self::Config(ConfigField::from(config)),
        }
    }
}

/// One declared field's full definition (mirror of `descriptor::field-config`).
#[derive(Debug, Clone, Serialize)]
#[allow(clippy::struct_excessive_bools)] // each bool mirrors an independent field-config toggle
pub struct ConfigField {
    pub name: String,
    pub fieldtype: Option<String>,
    pub label: Option<String>,
    pub options: Option<String>,
    pub reqd: bool,
    pub read_only: bool,
    pub read_only_on_load: bool,
    pub hidden: bool,
    pub columns: u8,
    pub permlevel: u8,
    pub placeholder: Option<String>,
    pub description: Option<String>,
    pub depends_on: Option<String>,
    pub min_value: Option<String>,
    pub max_value: Option<String>,
    pub precision: Option<u8>,
    pub default: Option<String>,
    /// Show the field in the Desk list view. Opt-in: a field that stays silent
    /// here is not listed, the same default the `#[doctype]` macro uses, so the
    /// list shows what the app deliberately put in it.
    pub in_list_view: Option<bool>,
}

impl From<&FieldConfig> for ConfigField {
    fn from(fc: &FieldConfig) -> Self {
        Self {
            name: fc.name.clone(),
            fieldtype: fc.fieldtype.clone(),
            label: fc.label.clone(),
            options: fc.options.clone(),
            reqd: fc.reqd,
            read_only: fc.read_only,
            read_only_on_load: fc.read_only_on_load,
            hidden: fc.hidden,
            columns: fc.columns,
            permlevel: fc.permlevel,
            placeholder: fc.placeholder.clone(),
            description: fc.description.clone(),
            depends_on: fc.depends_on.clone(),
            min_value: fc.min_value.clone(),
            max_value: fc.max_value.clone(),
            precision: fc.precision,
            default: fc.default.clone(),
            in_list_view: fc.in_list_view,
        }
    }
}

/// A declared index over the doctype's own fields (mirror of
/// `descriptor::index`).
#[derive(Debug, Clone, Serialize)]
pub struct ConfigIndex {
    pub fields: Vec<String>,
    pub unique: bool,
}

impl From<&Index> for ConfigIndex {
    fn from(index: &Index) -> Self {
        Self {
            fields: index.fields.clone(),
            unique: index.unique,
        }
    }
}

/// The (`view`, `dashboard`) pair a declared [`DocView`] means — the **only**
/// translation from the WIT cases to the host's metadata, shared by
/// [`doctype_meta`] and the endpoint mirror ([`ConfigDoctype`]) so the two
/// cannot read a declaration differently.
///
/// The WIT variant carries each dashboard binding *inside* its case, so an
/// ignored binding is unrepresentable on the way in; what crosses here is the
/// two-field shape the wire contract uses. A binding that names nothing
/// (`dashboard-fixed("")`) crosses verbatim: the Desk's own states report it
/// (missing panel / unlinked field) rather than the plugin silently loading
/// something the declaration did not say.
fn project_view(dt: &Doctype) -> (DocView, DashboardView) {
    match &dt.view {
        DeclaredView::Form => (DocView::Form, DashboardView::default()),
        DeclaredView::Custom => (DocView::Custom, DashboardView::default()),
        DeclaredView::DashboardNative => (DocView::Dashboard, DashboardView::default()),
        DeclaredView::DashboardFixed(id) => (
            DocView::Dashboard,
            DashboardView {
                dashboard: Some(id.clone()),
                ..DashboardView::default()
            },
        ),
        DeclaredView::DashboardLink(field) => (
            DocView::Dashboard,
            DashboardView {
                link_field: Some(field.clone()),
                ..DashboardView::default()
            },
        ),
        // The document binding: the guest names the doctype's own fields that
        // carry the dashboard format, and the host keeps those names verbatim
        // (the contract resolves each `None`/empty one to its canonical field).
        DeclaredView::DashboardDocument(format) => (
            DocView::Dashboard,
            DashboardView::document_fields(DocumentFormat {
                widgets: format.widgets.clone(),
                layout: format.layout.clone(),
                label: format.label.clone(),
                config: format.config.clone(),
            }),
        ),
    }
}

/// Build schema metadata for a plugin-declared `DocType`.
///
/// Capability flags (`allow_rename`/`allow_copy`/`allow_delete`/`allow_print`,
/// and the rest of the `allow_*` set) default closed. If the
/// app sets them in `describe()` they become UI affordances only —
/// authorization still flows through the engine's per-role permission checks
/// (fail-closed default-deny since C1).
pub fn doctype_meta(dt: &Doctype, module: &str) -> DoctypeMeta {
    let fields: Vec<FieldMeta> = dt.fields.iter().map(plugin_field_meta).collect();
    // What the declaration says about presentation: one match, one meaning.
    let (view, dashboard) = project_view(dt);
    // Index names are schema-wide in some dialects, so they carry the doctype
    // to stay unique across the app.
    let indexes: Vec<IndexMeta> = dt
        .indexes
        .iter()
        .enumerate()
        .map(|(i, index)| IndexMeta {
            name: format!("{}_{}", dt.name, i + 1),
            fields: index.fields.clone(),
            unique: index.unique,
            index_type: 0,
        })
        .collect();
    DoctypeMeta {
        name: dt.name.clone(),
        module: module.to_string(),
        fields,
        // A Single's one row is named after the DocType (see
        // `DoctypeMeta::single_name`), so it declares no naming series; every
        // other plugin doctype names from `<DOCTYPE>.<#####>`.
        naming_series: if dt.is_single {
            String::new()
        } else {
            format!("{}.#####", dt.name.to_uppercase())
        },
        is_submittable: dt.is_submittable,
        // A Single has no list view and no create affordance: it is reached
        // through its declared workspace link.
        is_single: dt.is_single,
        allow_submit: dt.allow_submit,
        allow_cancel: dt.allow_cancel,
        allow_amend: dt.allow_amend,
        allow_delete: dt.allow_delete,
        allow_copy: dt.allow_copy,
        allow_print: dt.allow_print,
        allow_rename: dt.allow_rename,
        in_create: !dt.is_single,
        in_list_view: !dt.is_single,
        in_dialog: true,
        in_global_search: !dt.is_single,
        in_calendar: dt.in_calendar,
        in_kanban: dt.in_kanban,
        in_gantt: dt.in_gantt,
        in_tree_view: dt.in_tree_view,
        in_report_view: dt.in_report_view,
        indexes,
        // `list-default-order` ("<field> <asc|desc>") is projected onto the
        // same sort metadata the `#[doctype]` macro can set. An unparseable
        // value is ignored (the host default applies), never an error.
        sort_field: dt.list_default_order.as_deref().and_then(list_order_field),
        sort_order: dt
            .list_default_order
            .as_deref()
            .and_then(list_order_direction),
        view,
        dashboard,
        ..DoctypeMeta::default()
    }
}

/// The field of a `"<field> <asc|desc>"` default list order.
fn list_order_field(raw: &str) -> Option<String> {
    raw.split_whitespace()
        .next()
        .filter(|field| !field.is_empty())
        .map(str::to_string)
}

/// The direction of a default list order; only an explicit `desc` is
/// returned (anything else falls back to the host's default).
fn list_order_direction(raw: &str) -> Option<String> {
    match raw.split_whitespace().nth(1) {
        Some(dir) if dir.eq_ignore_ascii_case("desc") => Some("desc".to_string()),
        _ => None,
    }
}

/// Layout fieldtypes that never participate in lists or standard filters.
fn is_layout_field(fieldtype: &str) -> bool {
    matches!(
        fieldtype,
        "Section Break" | "Column Break" | "Tab Break" | "HTML" | "Table"
    )
}

fn plugin_field_meta(input: &Field) -> FieldMeta {
    match input {
        Field::Name(name) => FieldMeta {
            fieldname: name.clone(),
            label: Some(name.clone()),
            fieldtype: infer_fieldtype(name),
            // A bare name declares no layout at all, and a field that declares
            // no list view is not listed.
            in_list_view: false,
            in_standard_filter: true,
            ..FieldMeta::default()
        },
        Field::Config(fc) => {
            let fieldtype = fc
                .fieldtype
                .clone()
                .unwrap_or_else(|| infer_fieldtype(&fc.name));
            let layout = is_layout_field(&fieldtype);
            FieldMeta {
                fieldname: fc.name.clone(),
                label: Some(fc.label.clone().unwrap_or_else(|| fc.name.clone())),
                fieldtype: fieldtype.clone(),
                options: fc.options.clone(),
                reqd: fc.reqd,
                read_only: fc.read_only,
                read_only_on_load: fc.read_only_on_load,
                hidden: fc.hidden,
                columns: fc.columns,
                permlevel: fc.permlevel,
                placeholder: fc.placeholder.clone(),
                description: fc.description.clone(),
                depends_on: fc.depends_on.clone(),
                min_value: fc.min_value.clone(),
                max_value: fc.max_value.clone(),
                precision: fc.precision,
                default: fc.default.clone(),
                // Opt-in, layout fields included: a field that says nothing
                // here is not listed, whichever type it is.
                in_list_view: fc.in_list_view.unwrap_or(false),
                in_standard_filter: !layout && matches!(fieldtype.as_str(), "Select" | "Check"),
                ..FieldMeta::default()
            }
        }
    }
}

/// Small, opinionated inference for plugin-declared fields; anything unknown
/// becomes a `Data` column so the data JSON blob stays the source of truth.
pub(super) fn infer_fieldtype(field: &str) -> String {
    let lower = field.to_ascii_lowercase();
    if lower == "date" || lower.ends_with("_date") {
        "Date".into()
    } else if lower == "status" {
        "Select".into()
    } else {
        "Data".into()
    }
}
