use leptos::prelude::*;
use leptos_router::components::A;

use crate::app::{WorkspaceItem, WorkspaceItemKind};
use crate::components::ui::icon::Icon;
use ferris_web_shared::url::safe_href;
use icons::{ChevronRight, ExternalLink};

/// Recursive sidebar link list (renders groups as collapsible dropdowns).
#[component]
pub(super) fn ItemList(
    items: Vec<WorkspaceItem>,
    expanded: ReadSignal<std::collections::HashSet<String>>,
    set_expanded: WriteSignal<std::collections::HashSet<String>>,
    current_route: Memo<String>,
    path_prefix: String,
    depth: usize,
) -> impl IntoView {
    view! {
        <div class="mt-0.5 space-y-0.5">
            {items.into_iter().enumerate().map(|(i, item)| {
                let key = format!("{path_prefix}::{depth}::{i}");
                let item_icon = item.icon;
                let label = item.label.clone();

                match item.kind {
                    WorkspaceItemKind::Group => {
                        let icon = item_icon;
                        let key_toggle = key.clone();
                        let is_open_key = key_toggle.clone();
                        let is_open = Memo::new(move |_| expanded.with(|s| s.contains(&is_open_key)));
                        let key_child = key.clone();
                        view! {
                            <div>
                                <button
                                    class="w-full flex items-center gap-2.5 px-2.5 py-1.5 text-xs font-medium ferris-desk-sidebar__item rounded-lg transition-colors max-md:py-3 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                                    on:click=move |_| {
                                        set_expanded.update(|s| {
                                            if s.contains(&key_toggle) {
                                                s.remove(&key_toggle);
                                            } else {
                                                s.insert(key_toggle.clone());
                                            }
                                        });
                                    }
                                >
                                    <Icon name=icon class="w-4 h-4 shrink-0" />
                                    <span class="truncate flex-1 text-left">{label.clone()}</span>
                                    <span
                                        class=move || format!(
                                            "inline-flex shrink-0 text-muted-foreground/70 transition-transform duration-200 {}",
                                            if is_open.get() { "rotate-90" } else { "" }
                                        )
                                    >
                                        <ChevronRight class="size-3.5" />
                                    </span>
                                </button>
                                {move || {
                                        let key_child = key_child.clone();
                                        let children = item.children.clone();
                                        is_open.get().then(move || view! {
                                            <div class=move || format!(
                                                "border-l border-sidebar-border {}",
                                                if depth == 0 { "ml-4 pl-2" } else { "ml-3 pl-2" }
                                            )>
                                                <ItemList
                                                    items=children
                                                    expanded=expanded
                                                    set_expanded=set_expanded
                                                    current_route=current_route.clone()
                                                    path_prefix=key_child
                                                    depth=depth + 1
                                                />
                                            </div>
                                        })
                                    }}
                            </div>
                        }
                        .into_any()
                    }
                    WorkspaceItemKind::Url => {
                        let icon = item_icon;
                        let href = item.href.as_deref().and_then(safe_href);
                        let open_new = item.open_in_new_tab;
                        // A workspace URL is metadata: `javascript:`/`data:` must
                        // never reach `href`, so an unsafe row keeps its label
                        // but loses the destination.
                        let Some(href) = href else {
                            return view! {
                                <span
                                    class="flex items-center gap-2.5 px-2.5 py-1.5 text-xs ferris-desk-sidebar__item rounded-lg max-md:py-3"
                                    title=label.clone()
                                >
                                    <Icon name=icon class="w-4 h-4 shrink-0" />
                                    <span class="truncate flex-1">{label.clone()}</span>
                                </span>
                            }
                            .into_any();
                        };
                        view! {
                            // This branch stays a native `<a>` on purpose: it can
                            // point off-origin, and `<A>` would try to route it
                            // through the SPA. But `target` must stay *empty*
                            // unless a new tab was asked for — leptos_router's
                            // delegated click handler skips any anchor with a
                            // non-empty `target`
                            // (leptos_router-0.8.15/src/location/mod.rs:336), and
                            // `target="_self"` was opting internal URLs out of
                            // client-side routing for no reason.
                            <a
                                href=href
                                target=move || if open_new { "_blank" } else { "" }
                                rel="noopener noreferrer"
                                class="flex items-center gap-2.5 px-2.5 py-1.5 text-xs ferris-desk-sidebar__item rounded-lg transition-colors max-md:py-3 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                                title=label.clone()
                            >
                                <Icon name=icon class="w-4 h-4 shrink-0" />
                                <span class="truncate flex-1">{label.clone()}</span>

                                <Show when=move || open_new>
                                    <ExternalLink class="size-4 shrink-0" />
                                </Show>
                            </a>
                        }
                        .into_any()
                    }
                    WorkspaceItemKind::Doctype | WorkspaceItemKind::Route => {
                        let icon = item_icon;
                        let route = item.route.as_deref().and_then(safe_href);
                        let is_active = Memo::new({
                            let route = route.clone();
                            let current_route = current_route.clone();
                            move |_| {
                                route.as_deref().is_some_and(|route| {
                                    route_is_active(&current_route.get(), route)
                                })
                            }
                        });
                        // `<A>` and not `<a>`. The router's delegated click handler
                        // was already routing these, so what this buys is
                        // `aria-current` on the active link and route resolution
                        // — not the navigation itself.
                        //
                        // `class` and `title` need the `attr:` prefix: on a
                        // component a bare `class=` is parsed as a Rust method call.
                        // Separate bindings, so the reactive `class` closure and the
                        // children do not both want to move `label`.
                        let title = label.clone();
                        let Some(route) = route else {
                            return view! {
                                <span
                                    class="flex items-center gap-2.5 px-2.5 py-1.5 text-xs ferris-desk-sidebar__item rounded-lg max-md:py-3"
                                    title=title
                                >
                                    <Icon name=icon class="w-4 h-4 shrink-0" />
                                    <span class="truncate flex-1">{label.clone()}</span>
                                </span>
                            }
                            .into_any();
                        };
                        view! {
                            <A
                                href=route
                                attr:class=move || format!(
                                    "flex items-center gap-2.5 px-2.5 py-1.5 text-xs rounded-lg transition-colors max-md:py-3 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 {}",
                                    if is_active.get() {
                                        "ferris-desk-sidebar__item--active"
                                    } else {
                                        "ferris-desk-sidebar__item"
                                    }
                                )
                                attr:title=title
                            >
                                <Icon name=icon class="w-4 h-4 shrink-0" />
                                <span class="truncate flex-1">{label.clone()}</span>
                            </A>
                        }
                        .into_any()
                    }
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}

/// Whether a link is the current desk location. Doctype links stay active on
/// their child routes (list -> form views).
pub(super) fn route_is_active(current: &str, target: &str) -> bool {
    current == target || (!target.is_empty() && current.starts_with(&format!("{target}/")))
}

/// The collapsed rail: one centred icon per top-level entry.
///
/// A leaf links with its label as the `title` tooltip. A group does *not* open
/// a floating menu: a 4rem rail has no room for a flyout that reads as part of
/// the navigation, so pressing a group unfolds its children **downward**,
/// inline — an accordion, the same tree the expanded sidebar draws, only the
/// icons survive. The open/closed state is the same `expanded` set the expanded
/// list uses, so folding a group survives toggling the sidebar.
#[component]
pub(super) fn CollapsedItemList(
    items: Vec<WorkspaceItem>,
    expanded: ReadSignal<std::collections::HashSet<String>>,
    set_expanded: WriteSignal<std::collections::HashSet<String>>,
    current_route: Memo<String>,
    path_prefix: String,
    depth: usize,
) -> impl IntoView {
    view! {
        <div class="mt-0.5 space-y-1">
            {items
                .into_iter()
                .enumerate()
                .map(|(i, item)| {
                    let key = format!("{path_prefix}::{depth}::{i}");
                    collapsed_entry(item, current_route, expanded, set_expanded, key, depth)
                })
                .collect_view()}
        </div>
    }
}

/// One collapsed-rail entry: its icon, wrapped in the control that makes sense
/// for its kind (a link for a leaf, a fold button for a group).
fn collapsed_entry(
    item: WorkspaceItem,
    current_route: Memo<String>,
    expanded: ReadSignal<std::collections::HashSet<String>>,
    set_expanded: WriteSignal<std::collections::HashSet<String>>,
    key: String,
    depth: usize,
) -> AnyView {
    let label = item.label.clone();
    let icon = item.icon.clone().unwrap_or_default();
    let size = rail_button_size(depth);

    match item.kind {
        WorkspaceItemKind::Group => {
            let key_toggle = key.clone();
            let is_open_key = key.clone();
            let is_open = Memo::new(move |_| expanded.with(|s| s.contains(&is_open_key)));

            // A folded group still lights up when one of its descendants names
            // the current route: without it the rail loses the "you are here"
            // cue the moment a branch is collapsed.
            let children_for_active = item.children.clone();
            let has_active =
                Memo::new(move |_| items_have_active(&children_for_active, &current_route.get()));

            let icon_btn = icon.clone();
            let label_btn = label.clone();
            let key_child = key.clone();
            view! {
                <div>
                    <button
                        type="button"
                        title=label_btn
                        aria-expanded=move || is_open.get().to_string()
                        class=move || format!(
                            "relative {size} mx-auto flex items-center justify-center rounded-lg transition-colors {}",
                            if is_open.get() || has_active.get() {
                                "ferris-desk-sidebar__item--active"
                            } else {
                                "ferris-desk-sidebar__item"
                            }
                        )
                        on:click=move |_| {
                            set_expanded.update(|s| {
                                if s.contains(&key_toggle) {
                                    s.remove(&key_toggle);
                                } else {
                                    s.insert(key_toggle.clone());
                                }
                            });
                        }
                    >
                        <Icon name=icon_btn class="size-4 shrink-0" />
                        <span
                            class=move || format!(
                                "pointer-events-none absolute right-0.5 bottom-0 inline-block text-muted-foreground/70 transition-transform duration-200 {}",
                                if is_open.get() { "rotate-90" } else { "" }
                            )
                        >
                            <ChevronRight class="size-2.5" />
                        </span>
                    </button>
                    {move || {
                        let children = item.children.clone();
                        let key_child = key_child.clone();
                        is_open.get().then(move || view! {
                            <div class=rail_children_class(depth)>
                                <CollapsedItemList
                                    items=children
                                    expanded=expanded
                                    set_expanded=set_expanded
                                    current_route=current_route
                                    path_prefix=key_child
                                    depth=depth + 1
                                />
                            </div>
                        })
                    }}
                </div>
            }
            .into_any()
        }
        WorkspaceItemKind::Url => {
            let href = item.href.as_deref().and_then(safe_href);
            let open_new = item.open_in_new_tab;
            let Some(href) = href else {
                return view! {
                    <span
                        title=label
                        class=format!(
                            "{size} mx-auto flex items-center justify-center ferris-desk-sidebar__item rounded-lg"
                        )
                    >
                        <Icon name=icon class="size-4 shrink-0" />
                    </span>
                }
                .into_any();
            };
            view! {
                // Native `<a>` and an empty `target` unless a new tab was
                // asked for, exactly like the expanded link: a non-empty
                // `target` would opt the URL out of SPA routing.
                <a
                    href=href
                    target=move || if open_new { "_blank" } else { "" }
                    rel="noopener noreferrer"
                    title=label
                    class=format!(
                        "{size} mx-auto flex items-center justify-center ferris-desk-sidebar__item rounded-lg transition-colors"
                    )
                >
                    <Icon name=icon class="size-4 shrink-0" />
                </a>
            }
            .into_any()
        }
        WorkspaceItemKind::Doctype | WorkspaceItemKind::Route => {
            let route = item.route.as_deref().and_then(safe_href);
            let is_active = Memo::new({
                let route = route.clone();
                move |_| {
                    route
                        .as_deref()
                        .is_some_and(|route| route_is_active(&current_route.get(), route))
                }
            });
            let title = label.clone();
            let Some(route) = route else {
                return view! {
                    <span
                        title=title
                        class=format!(
                            "{size} mx-auto flex items-center justify-center ferris-desk-sidebar__item rounded-lg"
                        )
                    >
                        <Icon name=icon class="size-4 shrink-0" />
                    </span>
                }
                .into_any();
            };
            view! {
                <A
                    href=route
                    attr:class=move || format!(
                        "{size} mx-auto flex items-center justify-center rounded-lg transition-colors {}",
                        if is_active.get() {
                            "ferris-desk-sidebar__item--active"
                        } else {
                            "ferris-desk-sidebar__item"
                        }
                    )
                    attr:title=title
                >
                    <Icon name=icon class="size-4 shrink-0" />
                </A>
            }
            .into_any()
        }
    }
}

/// Icon-tile size for a rail entry. The deeper the nesting, the smaller the
/// tile, so a folded subtree still fits the 4rem rail.
fn rail_button_size(depth: usize) -> &'static str {
    match depth {
        0 => "w-9 h-9",
        1 => "w-8 h-8",
        _ => "w-7 h-7",
    }
}

/// The fold line drawn to the left of a group's unfolded children: a hairline
/// plus the indent that makes the nesting legible in a column of icons.
fn rail_children_class(depth: usize) -> &'static str {
    match depth {
        0 => "ml-2 border-l border-sidebar-border pl-1",
        _ => "ml-1.5 border-l border-sidebar-border pl-1",
    }
}

/// Whether any entry names the current route, walking groups recursively. Used
/// so a folded group tile shows it has an active descendant.
fn items_have_active(items: &[WorkspaceItem], route: &str) -> bool {
    items.iter().any(|item| match item.kind {
        WorkspaceItemKind::Group => items_have_active(&item.children, route),
        WorkspaceItemKind::Doctype | WorkspaceItemKind::Route => item
            .route
            .as_deref()
            .map(|r| route_is_active(route, r))
            .unwrap_or(false),
        WorkspaceItemKind::Url => false,
    })
}
