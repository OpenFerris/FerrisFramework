//! One row of the tree and, when expanded, its subtree.
//!
//! Recursive DOM nesting (`role="group"`), not computed indentation: the
//! browser's layout and a screen reader agree on the depth without an inline
//! style (which the Desk forbids anyway).

use std::collections::BTreeSet;

use leptos::prelude::*;
use leptos_router::components::A;
use serde_json::Value;

use crate::api;
use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::icon::Icon;
use crate::desk::session::path_segment;

use super::model::TreeNode;

/// One row and, when expanded, its subtree.
///
/// Rendered recursively with nested containers instead of computed indentation:
/// the hierarchy is real DOM nesting (`role="group"`), so a screen reader and
/// the browser's own layout agree on the depth without an inline style.
pub(super) fn render_node(
    node: TreeNode,
    level: usize,
    expanded: &BTreeSet<String>,
    module: &str,
    doctype: &str,
    label_field: &str,
    toggle: Callback<String>,
) -> AnyView {
    let name = node.name.clone();
    let open = expanded.contains(&name);
    let has_children = !node.children.is_empty();
    let children_count = node.children.len();
    let label = api::display_value("Data", node.row.get(label_field).unwrap_or(&Value::Null));
    let label = if label.trim().is_empty() {
        name.clone()
    } else {
        label
    };
    let href = format!(
        "/desk/{}/{}/{}",
        path_segment(module),
        path_segment(doctype),
        path_segment(&name)
    );
    let toggle_name = name.clone();
    let aria_label = format!("{} «{label}»", if open { "Contraer" } else { "Expandir" });
    let subtree = (open && has_children).then(|| {
        node.children
            .into_iter()
            .map(|child| {
                render_node(
                    child,
                    level.saturating_add(1),
                    expanded,
                    module,
                    doctype,
                    label_field,
                    toggle,
                )
            })
            .collect_view()
    });

    view! {
        <div class="flex min-w-0 flex-col gap-1">
            <div
                role="treeitem"
                aria-expanded=has_children.then_some(open.to_string())
                data-name="TreeRow"
                data-document=name.as_str()
                data-level=level.to_string()
                class="flex min-w-0 items-center gap-1 rounded-md border border-transparent px-1 py-1 transition-colors hover:border-border hover:bg-muted/40 focus-within:border-border focus-within:bg-muted/40 max-md:min-h-10"
            >
                {has_children.then(|| {
                    let toggle_name = toggle_name.clone();
                    view! {
                        <Button
                            variant=ButtonVariant::Ghost
                            size=ButtonSize::Icon
                            attr:aria-label=aria_label
                            attr:aria-expanded=open.to_string()
                            on:click=move |_| toggle.run(toggle_name.clone())
                        >
                            <Icon
                                name=if open { "ChevronDown" } else { "ChevronRight" }
                                class="size-4 transition-transform duration-150 motion-reduce:transition-none"
                            />
                        </Button>
                    }
                })}
                {(!has_children).then(|| view! { <span class="inline-block size-9" aria-hidden="true"></span> })}
                <A
                    href=href
                    attr:class="min-w-0 flex-1 truncate rounded-sm text-sm font-medium text-foreground hover:underline focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                >
                    {label}
                </A>
                <Show when=move || has_children>
                    <Badge variant=BadgeVariant::Secondary size=BadgeSize::Sm>
                        {children_count}
                    </Badge>
                </Show>
                <span class="hidden shrink-0 text-xs text-muted-foreground sm:inline">{name.clone()}</span>
            </div>
            {subtree.map(|subtree| view! {
                <div role="group" class="ms-4 flex flex-col gap-1 border-s border-border/60 ps-2 sm:ms-5">
                    {subtree}
                </div>
            })}
        </div>
    }
    .into_any()
}
