mod header_dropdown;
mod item_list;

use header_dropdown::HeaderDropdown;
use item_list::{CollapsedItemList, ItemList, route_is_active};

use crate::WebUser;
use crate::app::{Workspace, WorkspaceItem, WorkspaceItemKind};
use crate::desk::UserMenu;
use crate::ext::{SLOT_SHELL_SIDEBAR_BOTTOM, UiSlot};
use icons::{Bell, BookOpen, PanelLeftClose, PanelLeftOpen, Search};
use leptos::prelude::*;
use leptos_router::components::A;

#[component]
pub fn Sidebar(
    workspaces: ReadSignal<Vec<Workspace>>,
    /// The workspace the user last picked, kept for the routes that belong to no
    /// workspace.
    picked: ReadSignal<Option<String>>,
    open: ReadSignal<bool>,
    on_toggle: WriteSignal<bool>,
    on_pin: Callback<String>,
    on_search: impl Fn() + 'static,
    on_notifications: impl Fn() + 'static,
    unread: Memo<u64>,
    current_route: Memo<String>,
) -> impl IntoView {
    // Which workspace the sidebar shows. A route that names exactly one
    // workspace (its links are the only ones that match) decides outright:
    // picking a workspace from the desk grid or the switcher navigates to its
    // first link, and a unique match is precisely what selects that workspace.
    // Apps can reuse routes across workspaces (the test app's three workspaces
    // all contain `/report/ops`, `/app/test/Project` and `/app/test/Task`), so
    // an ambiguous route must NOT fall back to the first match — that leaves
    // the sidebar stuck on the first workspace no matter what the user picks.
    // Ambiguous routes and routes that name no workspace at all (`/`, `/desk`,
    // `/settings`) therefore defer to the user's explicit `picked` workspace,
    // and only if there is none do they fall back to the first match. All the
    // reads have to be tracked, or the memo never invalidates and the sidebar
    // stays on whatever workspace happened to match when it was first read.
    let active_idx = Memo::new(move |_| {
        let list = workspaces.get();
        if list.is_empty() {
            return 0usize;
        }
        let route = current_route.get();

        let matches: Vec<usize> = list
            .iter()
            .enumerate()
            .filter(|(_, ws)| ws_items_match(&ws.items, &route))
            .map(|(i, _)| i)
            .collect();

        // A route names exactly one workspace: the deep link decides.
        if matches.len() == 1 {
            return matches[0];
        }
        // Ambiguous (several workspaces share the route) or no match at all:
        // the explicit choice decides, then the first match, then the top.
        picked
            .get()
            .and_then(|name| list.iter().position(|ws| ws.name == name))
            .or_else(|| matches.first().copied())
            .unwrap_or(0)
    });

    let (expanded, set_expanded) = signal(std::collections::HashSet::<String>::new());

    let on_reload = Callback::new(move |()| {
        if let Some(window) = web_sys::window() {
            let _ = window.location().reload();
        }
    });

    view! {
        <aside class=move || format!(
            "desk-sidebar relative flex-shrink-0 h-full transition-all duration-300 select-none {}",
            if open.get() {
                "w-[var(--sidebar-width)]"
            } else {
                "w-[var(--sidebar-width-icon)]"
            }
        )>
            <div class="ferris-desk-sidebar__panel flex flex-col h-full min-h-0 border-r">
                {/* 1. Module placeholder */}
                <div class=move || if open.get() { "p-4" } else { "p-3" }>
                    <HeaderDropdown
                        workspaces=workspaces
                        active_idx=active_idx
                        open=open
                        on_pin=on_pin
                        on_toggle_sidebar=on_toggle
                        on_reload=on_reload
                    />
                </div>

                {/* 2. Quick Actions: Search */}
                <div class="px-3 py-1 space-y-1 shrink-0">
                    <button
                        class=move || if open.get() {
                            "w-full flex items-center gap-2.5 px-2.5 py-1.5 text-xs font-medium ferris-desk-sidebar__item rounded-lg transition-colors max-md:py-3 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                        } else {
                            "w-9 h-9 mx-auto flex items-center justify-center ferris-desk-sidebar__item rounded-lg transition-colors focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                        }
                        on:click=move |_| on_search()
                        title="Buscar (Ctrl+K)"
                    >
                        <Search class="size-4 shrink-0" />
                        {move || open.get().then(|| view! {
                            <span>"Buscar"</span>
                        })}
                    </button>

                    <button
                        class=move || if open.get() {
                            "relative w-full flex items-center gap-2.5 px-2.5 py-1.5 text-xs font-medium ferris-desk-sidebar__item rounded-lg transition-colors max-md:py-3 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                        } else {
                            "relative w-9 h-9 mx-auto flex items-center justify-center ferris-desk-sidebar__item rounded-lg transition-colors focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                        }
                        on:click=move |_| on_notifications()
                        title="Avisos"
                    >
                        <Bell class="size-4 shrink-0" />
                        {move || {
                            let count = unread.get();
                            (count > 0).then(|| view! {
                                <span
                                    class="absolute right-1.5 top-1 h-1.5 w-1.5 rounded-full bg-primary"
                                    title=move || format!("{count} sin leer")
                                ></span>
                            })
                        }}
                        {move || open.get().then(|| view! {
                            <span>"Avisos"</span>
                        })}
                    </button>
                </div>

                <div class="my-2 border-b border-sidebar-border shrink-0 mx-3"></div>

                {/* 3. Navigation: the active workspace's links */}
                <div class=move || if open.get() {
                    "flex-1 min-h-0 overflow-y-auto px-3 space-y-0.5"
                } else {
                    "flex-1 min-h-0 overflow-y-auto px-1 space-y-0.5"
                }>
                    {move || {
                        let list = workspaces.get();
                        if list.is_empty() {
                            return view! { <span></span> }.into_any();
                        }
                        let idx = active_idx.get();
                        let ws = list[if idx < list.len() { idx } else { 0 }].clone();

                        // Collapsed to the rail, the labels do not fit: render
                        // one centred icon per entry, groups unfolding their
                        // children downward in place. The expanded tree keeps
                        // the inline, indented list — same `expanded` set, so a
                        // folded group survives the sidebar toggle.
                        if open.get() {
                            view! {
                                <ItemList
                                    items=ws.items
                                    expanded=expanded
                                    set_expanded=set_expanded
                                    current_route=current_route
                                    path_prefix=ws.name
                                    depth=0
                                />
                            }.into_any()
                        } else {
                            view! {
                                <CollapsedItemList
                                    items=ws.items
                                    expanded=expanded
                                    set_expanded=set_expanded
                                    current_route=current_route
                                    path_prefix=ws.name
                                    depth=0
                                />
                            }.into_any()
                        }
                    }}
                </div>

                {/* 4. OpenUI slot: plugin status dots and quick actions sit
                    between the nav and the footer, above the fold either way */}
                <div class="shrink-0 px-3 pb-1">
                    <UiSlot slot_name=SLOT_SHELL_SIDEBAR_BOTTOM />
                </div>

                {/* 5. Footer */}
                <div class="shrink-0 p-3 border-t border-sidebar-border space-y-2">
                    {move || if open.get() {
                        view! {
                            <div class="space-y-1">
                                // A real route, not the `<a href="#">` this row
                                // used to be: an href that goes nowhere still
                                // pushes a history entry the back button has to
                                // step through. `/settings/about` exists, so the
                                // label is a link like every other nav row.
                                <A
                                    href="/settings/about"
                                    attr:class="ferris-desk-sidebar__item flex items-center gap-2.5 px-2.5 py-1.5 text-xs font-medium rounded-lg transition-colors max-md:py-3 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                                >
                                    <BookOpen class="size-4 shrink-0" />
                                    <span>"Primeros pasos"</span>
                                </A>
                                <div class="pt-1">
                                    <UserMenu user={expect_context::<RwSignal<WebUser>>()} expanded=open />
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="flex flex-col items-center gap-2">
                                <UserMenu user={expect_context::<RwSignal<WebUser>>()} expanded=open />
                            </div>
                        }.into_any()
                    }}
                </div>
            </div>

            // The collapse handle: a hairline strip down the rail's right edge.
            // Clicking anywhere along it folds or unfolds the sidebar; on hover a
            // small round toggle appears in the middle, pointing the way the rail
            // will move.
            <button
                type="button"
                class="group/edge absolute inset-y-0 right-0 z-20 flex w-1 cursor-pointer items-center justify-center hover:bg-accent focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                on:click=move |_| on_toggle.update(|s| *s = !*s)
                title=move || if open.get() { "Contraer barra lateral" } else { "Expandir barra lateral" }
                aria-label="Alternar barra lateral"
            >
                <span class="pointer-events-none absolute top-1/2 left-1/2 flex size-6 -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-full border bg-background text-muted-foreground opacity-0 shadow-sm transition-opacity duration-200 group-hover/edge:opacity-100">
                    {move || if open.get() {
                        view! { <PanelLeftClose class="size-3.5" /> }.into_any()
                    } else {
                        view! { <PanelLeftOpen class="size-3.5" /> }.into_any()
                    }}
                </span>
            </button>
        </aside>
    }
}

/// Route-driven workspace matching (group items are walked recursively).
fn ws_items_match(items: &[WorkspaceItem], route: &str) -> bool {
    items.iter().any(|item| match item.kind {
        WorkspaceItemKind::Group => ws_items_match(&item.children, route),
        WorkspaceItemKind::Doctype | WorkspaceItemKind::Route => item
            .route
            .as_deref()
            .map(|r| route_is_active(route, r))
            .unwrap_or(false),
        WorkspaceItemKind::Url => false,
    })
}
