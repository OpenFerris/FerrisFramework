//! Adaptation of the declared themes, folders, workspaces, UI extensions,
//! dashboards and website onto the `ferris-core` registry specs.

use std::collections::HashMap;

use tracing::warn;

use ferris_core::themes::ThemeSpec;
use ferris_core::ui_extensions::{
    DashboardSpec, DashboardStyle, GridSizeSpec, StyleSpec, UiExtensionSpec, WebsiteRouteSpec,
    WebsiteSpec, WidgetDefSpec, WidgetInstanceSpec,
};
use ferris_core::workspaces::{WorkspaceFolderSpec, WorkspaceItemSpec, WorkspaceSpec};

use crate::ferris::framework::descriptor::{
    Dashboard, Folder, Theme, UiExtension, WebsiteRoute, Workspace, WorkspaceItem,
};

impl From<&Theme> for ThemeSpec {
    fn from(theme: &Theme) -> Self {
        Self {
            id: theme.id.clone(),
            label: theme.label.clone(),
            dark: theme.dark,
            vars: theme.vars.clone(),
        }
    }
}

impl From<&Folder> for WorkspaceFolderSpec {
    fn from(folder: &Folder) -> Self {
        Self {
            name: folder.name.clone(),
            label: folder.label.clone(),
            icon: folder.icon.clone(),
        }
    }
}

/// One declared workspace, with the flattened WIT item rows rebuilt into the
/// nested tree `ferris-core` expects.
#[must_use]
pub fn workspace_spec(workspace: &Workspace) -> WorkspaceSpec {
    WorkspaceSpec {
        name: workspace.name.clone(),
        label: workspace.label.clone(),
        module: workspace.module.clone(),
        icon: workspace.icon.clone(),
        folder: workspace.folder.clone(),
        items: workspace_item_tree(&workspace.items),
    }
}

/// One declared UI extension, adapted to the spec type `ferris-core`'s
/// registry (`register_app_ui`) stores and the Desk mounts into its slots.
#[must_use]
pub fn ui_extension_spec(extension: &UiExtension) -> UiExtensionSpec {
    UiExtensionSpec {
        slot: extension.slot.clone(),
        doctype: extension.doctype.clone(),
        component: extension.component.clone(),
        label: extension.label.clone(),
        props: extension.props.clone(),
        requires: extension.requires.clone(),
    }
}

/// The WIT footprint hint as the spec shape.
const fn grid_size_spec(size: crate::ferris::framework::descriptor::GridSize) -> GridSizeSpec {
    GridSizeSpec {
        span: size.span,
        rows: size.rows,
    }
}

/// The appearance a WIT `style-spec` declares, projected onto the registry's
/// spec type.
///
/// Field for field and pass-through: the WIT record already has every field
/// optional and the two vocabularies are the same one, so there is nothing to
/// translate — the function exists so the projection is named in one place and a
/// future type change breaks here rather than silently dropping a field.
fn style_spec(style: &crate::ferris::framework::descriptor::StyleSpec) -> StyleSpec {
    StyleSpec {
        background: style.background.clone(),
        background_custom: style.background_custom.clone(),
        border_color: style.border_color.clone(),
        border_width: style.border_width,
        border_radius: style.border_radius,
        padding: style.padding,
        shadow: style.shadow.clone(),
    }
}

/// The board appearance a WIT `dashboard-style` declares, projected onto the
/// registry's spec type. Same pass-through reasoning as [`style_spec`].
fn dashboard_style(style: &crate::ferris::framework::descriptor::DashboardStyle) -> DashboardStyle {
    DashboardStyle {
        background: style.background.clone(),
        background_custom: style.background_custom.clone(),
        gap_px: style.gap_px,
        padding_px: style.padding_px,
        border_radius: style.border_radius,
        title_full_screen: style.title_full_screen,
    }
}

/// One reusable widget definition, adapted to the spec type the registry
/// stores. These make up the app's contribution to the global widget catalog.
pub fn widget_def_spec(widget: &crate::ferris::framework::descriptor::WidgetDef) -> WidgetDefSpec {
    WidgetDefSpec {
        name: widget.name.clone(),
        label: widget.label.clone(),
        component: widget.component.clone(),
        code_path: widget.code_path.clone(),
        script: widget.script.clone(),
        stylesheet: widget.stylesheet.clone(),
        ssr: widget.ssr.clone(),
        renderer: widget.renderer.clone(),
        version: widget.version.clone(),
        kind: widget.kind.clone(),
        params: widget
            .params
            .iter()
            .map(|p| ferris_core::ui_extensions::WidgetParamSpec {
                name: p.name.clone(),
                label: p.label.clone(),
                value_type: p.value_type.clone(),
                default: p.default.clone(),
                options: p.options.clone(),
                required: p.required,
                description: p.description.clone(),
                group: p.group.clone(),
                placeholder: p.placeholder.clone(),
                depends_on: p.depends_on.clone(),
                read_only: p.read_only,
                hidden: p.hidden,
            })
            .collect(),
        description: widget.description.clone(),
        requires: widget.requires.clone(),
        events: widget.events.clone(),
        icon: widget.icon.clone(),
        category: widget.category.clone(),
        preview: widget.preview.clone(),
        size: widget.size.map(grid_size_spec),
        min: widget.min.map(grid_size_spec),
        style: widget.style.as_ref().map(style_spec),
    }
}

/// One widget *instance* inside a dashboard, adapted to the spec type the
/// registry stores (a reference plus its per-placement configuration).
fn widget_instance_spec(
    instance: &crate::ferris::framework::descriptor::WidgetInstance,
) -> WidgetInstanceSpec {
    WidgetInstanceSpec {
        id: instance.id.clone(),
        widget: instance.widget.clone(),
        label: instance.label.clone(),
        params: instance.params.clone(),
        span: instance.span,
        col: instance.col,
        row: instance.row,
        rows: instance.rows,
        style: instance.style.as_ref().map(style_spec),
    }
}

/// One declared dashboard, adapted to the spec type the registry stores.
pub fn dashboard_spec(dashboard: &Dashboard) -> DashboardSpec {
    DashboardSpec {
        id: dashboard.id.clone(),
        label: dashboard.label.clone(),
        route: dashboard.route.clone(),
        default: dashboard.default,
        style: dashboard.style.as_ref().map(dashboard_style),
        widgets: dashboard.widgets.iter().map(widget_instance_spec).collect(),
    }
}

/// The app's website publishing declaration, adapted to the spec type the
/// registry stores.
///
/// The descriptor carries `website: option<list<
/// website-route>>`: there is no wrapper object left, so an empty declaration
/// and "no website" are distinguished by the descriptor's `Option`, not here.
#[must_use]
pub fn website_spec(routes: &[WebsiteRoute]) -> WebsiteSpec {
    WebsiteSpec {
        routes: routes
            .iter()
            .map(|route| WebsiteRouteSpec {
                path: route.path.clone(),
                component: route.component.clone(),
                kind: route.kind.clone(),
            })
            .collect(),
    }
}

/// Rebuild the workspace item tree from the flattened `id`/`parent` rows.
///
/// The component model forbids recursive types, so the contract carries rows
/// instead of nested children. Rows are ordered and a `parent` must refer to a
/// row id that appears **earlier**; a row whose parent is dangling or forward
/// is treated as top-level rather than dropped (`register_app_workspaces`
/// normalizes what it stores, and the desk renders the icon on the grid).
fn workspace_item_tree(rows: &[WorkspaceItem]) -> Vec<WorkspaceItemSpec> {
    let specs: Vec<WorkspaceItemSpec> = rows
        .iter()
        .map(|row| WorkspaceItemSpec {
            label: row.label.clone(),
            icon: row.icon.clone(),
            doctype: row.doctype.clone(),
            route: row.route.clone(),
            href: row.href.clone(),
            open_in_new_tab: row.open_in_new_tab,
            children: Vec::new(),
        })
        .collect();

    // Row index by declared id so `parent` links resolve to positions. Rows
    // are ordered, so a parent always precedes its children.
    let index_of: HashMap<u32, usize> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| (row.id, i))
        .collect();
    // child spec index -> parent spec index, for every *valid* link.
    let mut parents: HashMap<usize, usize> = HashMap::new();
    for (i, row) in rows.iter().enumerate() {
        match row.parent {
            None => {}
            Some(parent_id) => match index_of.get(&parent_id).copied() {
                Some(parent) if parent < i => {
                    parents.insert(i, parent);
                }
                _ => {
                    warn!(
                        workspace_item = %row.label,
                        parent = parent_id,
                        "workspace item parent is dangling or not declared earlier; treating it as top-level"
                    );
                }
            },
        }
    }

    fn build(
        i: usize,
        specs: &[WorkspaceItemSpec],
        parents: &HashMap<usize, usize>,
    ) -> WorkspaceItemSpec {
        let mut spec = specs[i].clone();
        for (child, parent) in parents {
            if *parent == i {
                spec.children.push(build(*child, specs, parents));
            }
        }
        spec
    }

    let mut roots = Vec::new();
    for i in 0..specs.len() {
        if !parents.contains_key(&i) {
            roots.push(build(i, &specs, &parents));
        }
    }
    roots
}
