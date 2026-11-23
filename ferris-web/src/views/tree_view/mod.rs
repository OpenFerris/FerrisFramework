//! Tree view: records nested by a `Link` field the doctype declares as its
//! parent (`DoctypeMeta::tree_parent_field`).
//!
//! The tree is **metadata-driven end to end**: which field nests the rows, which
//! field labels them and how many levels open on first paint all come from the
//! doctype definition (compiled or a runtime `Property Setter`). A doctype that
//! declares `in_tree_view` but not a parent field gets an explanation and a link
//! to its settings instead of an invented hierarchy.
//!
//! The expansion state is persisted per module/doctype, so a reload or a return
//! to the page reopens the branches the user left open. It is written only by an
//! effect, never during SSR, which keeps the server render and the hydrated
//! render identical.
//!
//! Cycles are refused at render time, not assumed away: a document whose parent
//! chain points back at itself becomes a root instead of disappearing from the
//! page (the attachment SQL has no cycle constraint, and a hand-edited row must
//! not take the whole tree down with it).

mod model;
mod node;

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use serde_json::Value;

use crate::api::hooks::use_persisted_view_state;
use crate::api::{self, DocStore, ListParams};
use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::components::ui::card::{Card, CardContent};
use crate::components::ui::icon::Icon;
use crate::desk::session::path_segment;
use crate::i18n::use_i18n;
use crate::views::views_components::collection::{
    stale_settings_message_i18n, unresolved_fields_i18n, use_collection_actions,
};
use crate::views::views_components::{
    EmptyState, Feedback, FeedbackKind, LoadingLines, PageHeader,
};
use model::TreeState;
pub(crate) use model::conventional_parent;
pub use model::{TreeNode, build_tree, count_nodes, default_expanded, filter_tree};
use node::render_node;

/// Rows fetched per request while walking the doctype.
const PAGE: u64 = 500;
/// Stop after this many pages: a tree over more rows is a report, not a tree.
const MAX_PAGES: u64 = 5;

#[component]
pub fn TreeView() -> impl IntoView {
    let params = use_params_map();
    let i18n = use_i18n();
    let i18n_sv = i18n;
    let tr = move |key: &str| {
        i18n_sv
            .try_with_value(|i| i.t(key))
            .unwrap_or_else(|| key.to_string())
    };
    let doctype = Signal::derive(move || params.with(|p| p.get("doctype").unwrap_or_default()));

    let meta = RwSignal::new(None::<crate::api::DynMeta>);
    let rows = RwSignal::new(Vec::<Value>::new());
    let total = RwSignal::new(0u64);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(None::<String>);
    let reload = RwSignal::new(0u64);
    let search = RwSignal::new(String::new());

    let key = Signal::derive(move || {
        format!(
            "ferris.tree.{}.{}",
            params.with(|p| p.get("module").unwrap_or_default()),
            doctype.get()
        )
    });
    let state = use_persisted_view_state::<TreeState>(key);

    // ── Fetch metadata + every page the tree can hold ───────────────────
    Effect::new(move |_| {
        let dt = doctype.get();
        let _ = reload.get();
        if dt.is_empty() {
            rows.set(Vec::new());
            meta.set(None);
            loading.set(false);
            return;
        }
        loading.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            let store = match DocStore::runtime() {
                Ok(store) => store,
                Err(message) => {
                    error.set(Some(message));
                    loading.set(false);
                    return;
                }
            };
            let found = match store.meta(&dt).await {
                Ok(found) => found,
                Err(message) => {
                    error.set(Some(message));
                    loading.set(false);
                    return;
                }
            };
            meta.set(found);
            let mut all: Vec<Value> = Vec::new();
            let mut total_rows = 0u64;
            for page in 0..MAX_PAGES {
                let params = ListParams {
                    limit_start: page * PAGE,
                    limit_page_length: PAGE,
                    filters: None,
                    order_by: Some("creation asc".into()),
                    fields: None,
                    only_liked: None,
                    search: None,
                };
                match store.list(&dt, &params).await {
                    Ok(response) => {
                        total_rows = response.total;
                        let empty = response.data.is_empty();
                        all.extend(response.data);
                        if empty || u64::try_from(all.len()).unwrap_or(u64::MAX) >= total_rows {
                            break;
                        }
                    }
                    Err(message) => {
                        error.set(Some(message));
                        break;
                    }
                }
            }
            total.set(total_rows);
            rows.set(all);
            loading.set(false);
        });
    });

    let meta_sig = meta.read_only();
    let on_reload = Callback::new(move |()| reload.update(|n| *n += 1));
    use_collection_actions(meta_sig, "tree", on_reload);

    let i18n_stale = i18n;
    let stale_settings = Memo::new(move |_| {
        meta.get().as_ref().map_or_else(Vec::new, |meta| {
            unresolved_fields_i18n(
                meta,
                &[
                    ("views.tree.parent_field", meta.tree_parent_field.as_deref()),
                    ("views.tree.label_field", meta.tree_label_field.as_deref()),
                ],
                &i18n_stale,
            )
        })
    });

    // ── Derived configuration ───────────────────────────────────────────
    let parent_field = Memo::new(move |_| {
        meta.get().as_ref().and_then(|meta| {
            meta.tree_parent_field
                .clone()
                .filter(|field| !field.is_empty())
                // A doctype that declares the flag but has no stored parent
                // still works when it follows the conventional `parent` Link to
                // itself: that is the field Frappe's own tree doctypes use.
                .or_else(|| conventional_parent(meta))
        })
    });
    let label_field = Memo::new(move |_| {
        meta.get().as_ref().map_or_else(
            || "name".to_string(),
            |meta| {
                meta.tree_label_field
                    .clone()
                    .filter(|field| !field.is_empty())
                    .unwrap_or_else(|| api::title_field(meta).to_string())
            },
        )
    });
    let module = Memo::new(move |_| {
        meta.get()
            .as_ref()
            .map(|meta| meta.module.clone())
            .filter(|module| !module.is_empty())
            .unwrap_or_else(|| params.with(|p| p.get("module").unwrap_or_default()))
    });

    let tree = Memo::new(move |_| {
        let Some(field) = parent_field.get() else {
            return Vec::new();
        };
        build_tree(&rows.get(), &field)
    });

    // Seed the first-open expansion from the declared depth. Runs once per
    // view key: after that, the user's own expansion is the truth.
    Effect::new(move |_| {
        let roots = tree.get();
        if roots.is_empty() || state.get_untracked().initialized {
            return;
        }
        let levels = meta
            .get()
            .as_ref()
            .and_then(|meta| meta.tree_expanded_levels)
            .unwrap_or(1);
        let expanded = default_expanded(&roots, levels);
        state.update(|stored| {
            stored.expanded = expanded;
            stored.initialized = true;
        });
    });

    let visible = Memo::new(move |_| filter_tree(tree.get(), &search.get(), &label_field.get()));
    let expanded = Memo::new(move |_| {
        if search.get().trim().is_empty() {
            state.get().expanded
        } else {
            // A search shows the path to every match, so every branch that
            // survived the filter is open: collapsing them would leave the
            // matches themselves hidden behind a closed ancestor.
            default_expanded(&visible.get(), u8::MAX)
        }
    });
    let searching = Memo::new(move |_| !search.get().trim().is_empty());
    let shown = Memo::new(move |_| count_nodes(&visible.get()));

    let toggle = Callback::new(move |name: String| {
        state.update(|stored| {
            if !stored.expanded.remove(&name) {
                stored.expanded.insert(name);
            }
        });
    });

    let module_value = move || module.get();
    let doctype_value = move || doctype.get();
    let label = move || label_field.get();
    let parent = move || parent_field.get();
    let loading_state = loading;
    let error_state = error;
    let total_state = total;
    let rows_state = rows;
    let i18n_stale_msg = i18n;
    let i18n_settings = i18n;
    let i18n_docs = i18n;
    let i18n_matches = i18n;
    let i18n_render = i18n;

    view! {
        <div class="mx-auto flex h-full min-h-0 w-full min-w-0 flex-col gap-4 p-4">
            {move || view! {
                <PageHeader
                    title=doctype_value()
                    description=tr("views.tree.description")
                >
                    <div class="flex w-full min-w-0 items-center gap-2 sm:w-auto">
                        <label class="sr-only" for="tree-search">{tr("views.tree.search_label")}</label>
                        <input
                            id="tree-search"
                            type="search"
                            placeholder=tr("views.common.search_placeholder")
                            class="h-9 w-full rounded-md border border-input bg-background px-3 text-sm text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 sm:w-56"
                            prop:value=move || search.get()
                            on:input=move |ev| search.set(event_target_value(&ev))
                        />
                    </div>
                </PageHeader>
            }}

            <Feedback kind=FeedbackKind::Error message=Signal::derive(move || error_state.get()) />
            <Feedback
                kind=FeedbackKind::Warning
                message=Signal::derive(move || {
                    stale_settings_message_i18n(&stale_settings.get(), &i18n_stale_msg)
                })
            />

            <Show when=move || loading_state.get()>
                <Card>
                    <CardContent><LoadingLines lines=6 /></CardContent>
                </Card>
            </Show>

            <Show when=move || !loading_state.get() && error_state.get().is_none() && parent().is_none()>
                <EmptyState
                    icon="FolderTree"
                    title=tr("views.tree.missing_parent_title")
                    description=tr("views.tree.missing_parent_description")
                >
                    {{
                        let i18n_settings = i18n_settings;
                        move || {
                            let href = format!(
                                "/desk/{}/{}/layout",
                                path_segment(&module_value()),
                                path_segment(&doctype_value())
                            );
                            view! {
                                <A
                                    href=href
                                    attr:class="inline-flex h-9 items-center gap-2 rounded-md bg-primary px-4 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                                >
                                    <Icon name="Settings2" class="size-4" />
                                    {i18n_settings.t("views.common.open_doctype_settings")}
                                </A>
                            }
                        }
                    }}
                </EmptyState>
            </Show>

            <Show when=move || {
                !loading_state.get()
                    && error_state.get().is_none()
                    && parent().is_some()
                    && rows_state.with(Vec::is_empty)
            }>
                <EmptyState
                    icon="FolderTree"
                    title=tr("views.common.no_documents")
                    description=tr("views.tree.empty_description")
                />
            </Show>

            <Show when=move || {
                !loading_state.get()
                    && error_state.get().is_none()
                    && parent().is_some()
                    && !rows_state.with(Vec::is_empty)
            }>
                <div class="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                    <span>{{ let i18n_docs = i18n_docs; move || i18n_docs.t_with("views.common.documents", &[("count", &total_state.get().to_string())]) }}</span>
                    <Show when=move || searching.get()>
                        <Badge variant=BadgeVariant::Secondary size=BadgeSize::Sm>
                            {{ let i18n_matches = i18n_matches; move || i18n_matches.t_with("views.tree.matches", &[("count", &shown.get().to_string())]) }}
                        </Badge>
                    </Show>
                    <Show when=move || { total_state.get() > PAGE * MAX_PAGES }>
                        <Badge variant=BadgeVariant::Warning size=BadgeSize::Sm>
                            {tr("views.tree.truncated")}
                        </Badge>
                    </Show>
                </div>

                <Card class="min-h-0 flex-1 overflow-y-auto p-0">
                    <CardContent class="flex flex-col gap-1 p-2 sm:p-3">
                        <Show
                            when=move || !visible.get().is_empty()
                            fallback={{
                                let i18n = i18n;
                                move || view! {
                                    <EmptyState
                                        icon="Search"
                                        title=i18n.t("views.tree.no_matches_title")
                                        description=i18n.t("views.tree.no_matches_description")
                                    />
                                }
                            }}
                        >
                            {{
                                let i18n_render = i18n_render;
                                move || {
                                    let roots = visible.get();
                                    let expanded = expanded.get();
                                    let module = module_value();
                                    let doctype = doctype_value();
                                    let label = label();
                                    let i18n_node = i18n_render;
                                    let children = roots
                                        .into_iter()
                                        .map(|node| {
                                            render_node(
                                                node,
                                                1,
                                                &expanded,
                                                &module,
                                                &doctype,
                                                &label,
                                                toggle,
                                                &i18n_node,
                                            )
                                        })
                                        .collect_view();
                                    view! { <div role="tree" aria-label=i18n_render.t("views.tree.aria") class="flex flex-col gap-1">{children}</div> }
                                }
                            }}
                        </Show>
                    </CardContent>
                </Card>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests;
