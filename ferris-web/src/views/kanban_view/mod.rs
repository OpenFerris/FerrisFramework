//! Kanban view: records grouped by a metadata-chosen field.
//!
//! The column field comes from the doctype definition
//! (`DoctypeMeta::kanban_group_field`): a multi-value `Select` (columns are its
//! options, in their declared order) or a `Link` (columns are the values present
//! in the loaded rows). Without a declaration the board keeps the historical
//! derivation — the first multi-value `Select` — and a doctype with neither is
//! shown read-only over `docstatus`, because a lifecycle column is changed
//! through submit/cancel, never through an `update`.
//!
//! The board is a **write** surface, so the move is honest: a drop (or its
//! keyboard arrows) is applied optimistically to the card, the write is awaited,
//! and a refused write clears the optimistic position and shows the server's
//! message. A successful write reloads the page of records; nothing is assumed.
//!
//! A doctype with no grouping field authorizes no move: the arrows are hidden
//! and the columns are read-only instead of offering a write the engine will
//! reject.

mod model;

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use serde_json::Value;

use crate::api::{self, DocStore, ListParams};
use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::card::Card;
use crate::components::ui::icon::Icon;
use crate::desk::session::path_segment;
use crate::i18n::use_i18n;
use crate::views::views_components::collection::{
    stale_settings_message_i18n, unresolved_fields_i18n, use_collection_actions,
};
use crate::views::views_components::{
    EmptyState, Feedback, FeedbackKind, LoadingLines, PageHeader,
};
pub use model::{EMPTY_COLUMN, GroupSpec, column_key, columns, neighbour};
pub(crate) use model::{default_group, write_group};

/// Rows read for one board. A board over more documents is a report.
pub const PAGE: u64 = 1000;

// The drag handlers need their own copy of the column label; clippy's
// `redundant_clone` does not see the capture through the two closures.
#[allow(clippy::redundant_clone)]
#[component]
pub fn KanbanView() -> impl IntoView {
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
    // The card whose move is in flight; disables further moves so a double
    // click cannot write twice.
    let moving = RwSignal::new(None::<String>);
    // Optimistic positions: `name -> column`. Cleared when the write lands (the
    // reload is the truth) or when it fails (the card goes back).
    let optimistic = RwSignal::new(HashMap::<String, String>::new());
    let drag_over = RwSignal::new(None::<String>);

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
        let params = ListParams {
            limit_start: 0,
            limit_page_length: PAGE,
            filters: None,
            order_by: Some("creation asc".into()),
            fields: None,
            only_liked: None,
            search: None,
        };
        spawn_local(async move {
            let store = match DocStore::runtime() {
                Ok(store) => store,
                Err(message) => {
                    error.set(Some(message));
                    loading.set(false);
                    return;
                }
            };
            match store.meta(&dt).await {
                Ok(found) => meta.set(found),
                Err(message) => error.set(Some(message)),
            }
            match store.list(&dt, &params).await {
                Ok(response) => {
                    total.set(response.total);
                    rows.set(response.data);
                    // The page now carries the moves that were accepted; the
                    // optimistic overlay has done its job.
                    optimistic.set(HashMap::new());
                }
                Err(message) => error.set(Some(message)),
            }
            loading.set(false);
        });
    });

    let on_reload = Callback::new(move |()| {
        optimistic.set(HashMap::new());
        reload.update(|count| *count += 1);
    });
    use_collection_actions(meta.read_only(), "kanban", on_reload);

    let i18n_stale = i18n;
    let stale_settings = Memo::new(move |_| {
        meta.get().as_ref().map_or_else(Vec::new, |meta| {
            unresolved_fields_i18n(
                meta,
                &[(
                    "views.kanban.group_field",
                    meta.kanban_group_field.as_deref(),
                )],
                &i18n_stale,
            )
        })
    });

    let group = Memo::new(move |_| {
        meta.get()
            .as_ref()
            .map_or_else(default_group, GroupSpec::from_meta)
    });
    let title_field = Memo::new(move |_| {
        meta.get().as_ref().map_or_else(
            || "name".to_string(),
            |meta| api::title_field(meta).to_string(),
        )
    });
    let card_fields = Memo::new(move |_| {
        meta.get().as_ref().map_or_else(Vec::new, |meta| {
            if meta.kanban_card_fields.is_empty() {
                return Vec::new();
            }
            meta.kanban_card_fields
                .iter()
                .filter_map(|name| meta.fields.iter().find(|field| &field.fieldname == name))
                .map(|field| {
                    (
                        api::label_of(field),
                        field.fieldname.clone(),
                        field.fieldtype.clone(),
                    )
                })
                .collect()
        })
    });
    let module = Memo::new(move |_| {
        meta.get()
            .as_ref()
            .map(|meta| meta.module.clone())
            .filter(|module| !module.is_empty())
            .unwrap_or_else(|| params.with(|p| p.get("module").unwrap_or_default()))
    });

    let i18n_move = i18n;
    let move_card = {
        Callback::new(move |(name, target): (String, String)| {
            if moving.get_untracked().is_some() {
                return;
            }
            let dt = doctype.get_untracked();
            let spec = group.get_untracked();
            if !spec.movable || dt.is_empty() {
                return;
            }
            error.set(None);
            moving.set(Some(name.clone()));
            optimistic.update(|moved| {
                moved.insert(name.clone(), target.clone());
            });
            let i18n_write = i18n_move;
            spawn_local(async move {
                let write = write_group(&dt, &spec.field, &name, &target).await;
                match write {
                    Ok(()) => {
                        // The optimistic column stays until the refreshed page
                        // lands (the fetch effect clears it): removing it here
                        // would snap the card back to the old column for as
                        // long as the re-read takes.
                        reload.update(|count| *count += 1);
                    }
                    Err(message) => {
                        // Revert: the card goes back to the column the server
                        // still has, and the message says why.
                        optimistic.update(|moved| {
                            moved.remove(&name);
                        });
                        error.set(Some(i18n_write.t_with(
                            "views.kanban.move_failed",
                            &[("name", &name), ("target", &target), ("message", &message)],
                        )));
                    }
                }
                moving.set(None);
            });
        })
    };

    let i18n_showing = i18n;
    let i18n_stale_msg = i18n;
    let i18n_view = i18n;
    view! {
        <div class="mx-auto flex h-full min-h-0 w-full min-w-0 flex-col gap-4 p-4">
            {move || view! {
                <PageHeader
                    title=doctype.get()
                    description=tr("views.kanban.description")
                />
            }}

            <Feedback kind=FeedbackKind::Error message=Signal::derive(move || error.get()) />
            <Feedback
                kind=FeedbackKind::Warning
                message=Signal::derive(move || {
                    stale_settings_message_i18n(&stale_settings.get(), &i18n_stale_msg)
                })
            />

            <Show when=move || loading.get()>
                <Card>
                    <crate::components::ui::card::CardContent>
                        <LoadingLines lines=4 />
                    </crate::components::ui::card::CardContent>
                </Card>
            </Show>

            <Show when=move || { total.get() > PAGE && !loading.get() }>
                <p class="text-xs text-muted-foreground">
                    {move || i18n_showing.t_with(
                        "views.kanban.showing",
                        &[("page", &PAGE.to_string()), ("total", &total.get().to_string())],
                    )}
                </p>
            </Show>

            {move || {
                if loading.get() || error.get().is_some() {
                    return AnyView::from(Fragment::new(Vec::new()));
                }
                if rows.with(Vec::is_empty) {
                    return view! {
                        <EmptyState
                            icon="SquareKanban"
                            title=i18n_view.t("views.common.no_documents")
                            description=i18n_view.t("views.kanban.empty_description")
                        />
                    }
                    .into_any();
                }
                let spec = group.get();
                let labels: Vec<String> = if spec.movable && !spec.options.is_empty() {
                    let mut declared = spec.options.clone();
                    // A value the data carries but the declaration does not
                    // list still needs a column to move to.
                    for row in rows.get() {
                        let key = column_key(&row, &spec.field);
                        if key != EMPTY_COLUMN && !declared.contains(&key) {
                            declared.push(key);
                        }
                    }
                    declared
                } else {
                    columns(&rows.get(), &spec, &optimistic.get())
                        .into_iter()
                        .map(|(key, _)| key)
                        .filter(|key| key != EMPTY_COLUMN)
                        .collect()
                };
                let ordered = columns(&rows.get(), &spec, &optimistic.get());
                let title = title_field.get();
                let extra = card_fields.get();
                let module = path_segment(&module.get());
                let doctype_segment = path_segment(&doctype.get());
                let movable = spec.movable;
                let column_views: Vec<AnyView> = ordered
                    .into_iter()
                    .map(|(label, cards)| {
                        let card_count = cards.len();
                        let labels_by_col = labels.clone();
                        let lab = label.clone();
                        let prev = neighbour(&lab, &labels_by_col, false);
                        let next = neighbour(&lab, &labels_by_col, true);
                        let title = title.clone();
                        let extra = extra.clone();
                        let module = module.clone();
                        let doctype_segment = doctype_segment.clone();
                        let drop_label = lab;
                        let cards: Vec<AnyView> = cards
                            .into_iter()
                            .map(|card| {
                                let cname = api::record_name(&card);
                                let display = api::display_value(
                                    "Data",
                                    card.get(&title).unwrap_or(&Value::Null),
                                );
                                let url = format!(
                                    "/desk/{module}/{doctype_segment}/{}",
                                    path_segment(&cname)
                                );
                                let created: String = card
                                    .get("creation")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .chars()
                                    .take(10)
                                    .collect();
                                let details: Vec<(String, String)> = extra
                                    .iter()
                                    .filter(|(_, name, _)| name != &title && name != "name" && name != "creation")
                                    .map(|(label, name, fieldtype)| {
                                        (
                                            label.clone(),
                                            api::display_value(
                                                fieldtype,
                                                card.get(name).unwrap_or(&Value::Null),
                                            ),
                                        )
                                    })
                                    .filter(|(_, value)| !value.trim().is_empty())
                                    .collect();
                                let details_view = (!details.is_empty()).then(|| {
                                    view! {
                                        <dl class="flex min-w-0 flex-col gap-0.5">
                                            {details.into_iter().map(|(label, value)| view! {
                                                <div class="flex min-w-0 items-baseline justify-between gap-2 text-xs">
                                                    <dt class="shrink-0 text-muted-foreground">{label}</dt>
                                                    <dd class="min-w-0 truncate text-foreground">{value}</dd>
                                                </div>
                                            }).collect_view()}
                                        </dl>
                                    }
                                });
                                let is_moving = {
                                    let cname = cname.clone();
                                    move || moving.get().as_deref() == Some(cname.as_str())
                                };
                                let prev_target = StoredValue::new(prev.clone());
                                let next_target = StoredValue::new(next.clone());
                                let card_name = StoredValue::new(cname.clone());
                                let has_prev = prev.is_some();
                                let has_next = next.is_some();
                                let prev_aria = StoredValue::new(
                                    prev.as_ref().map(|target| {
                                        i18n_view.t_with("views.kanban.move_to", &[("target", target)])
                                    }),
                                );
                                let next_aria = StoredValue::new(
                                    next.as_ref().map(|target| {
                                        i18n_view.t_with("views.kanban.move_to", &[("target", target)])
                                    }),
                                );
                                let drag_name = StoredValue::new(cname.clone());
                                view! {
                                    <div
                                        class="flex flex-col gap-1 rounded-md border border-border bg-card p-2 shadow-xs transition-[opacity,box-shadow] duration-150 motion-reduce:transition-none hover:shadow-sm focus-within:ring-[3px] focus-within:ring-ring/50"
                                        draggable=movable.then_some("true")
                                        data-name="KanbanCard"
                                        data-document=cname.as_str()
                                        on:dragstart=move |ev| {
                                            if let Some(dt) = ev.data_transfer() {
                                                let _ = dt.set_data("text/plain", &drag_name.get_value());
                                            }
                                        }
                                    >
                                        <div class="flex min-w-0 items-start justify-between gap-2">
                                            <A href=url attr:class="min-w-0 truncate text-sm font-medium hover:underline focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50">
                                                {display}
                                            </A>
                                            <Show when=move || is_moving()>
                                                <span class="shrink-0 text-[11px] text-muted-foreground" role="status">
                                                    {tr("views.kanban.moving")}
                                                </span>
                                            </Show>
                                        </div>
                                        {details_view}
                                        <div class="mt-1 flex min-w-0 items-center justify-between gap-2">
                                            <span class="min-w-0 truncate text-xs text-muted-foreground">{created}</span>
                                            <Show when=move || movable>
                                                <div class="flex shrink-0 items-center gap-1">
                                                    <Show when=move || has_prev>
                                                        <Button
                                                            variant=ButtonVariant::Ghost
                                                            size=ButtonSize::Icon
                                                            attr:disabled=move || moving.get().is_some()
                                                            attr:aria-label=move || prev_aria.get_value()
                                                            on:click=move |_| {
                                                                if let Some(target) = prev_target.get_value() {
                                                                    move_card.run((card_name.get_value(), target));
                                                                }
                                                            }
                                                        >
                                                            <Icon name="ChevronLeft" class="size-4" />
                                                        </Button>
                                                    </Show>
                                                    <Show when=move || has_next>
                                                        <Button
                                                            variant=ButtonVariant::Ghost
                                                            size=ButtonSize::Icon
                                                            attr:disabled=move || moving.get().is_some()
                                                            attr:aria-label=move || next_aria.get_value()
                                                            on:click=move |_| {
                                                                if let Some(target) = next_target.get_value() {
                                                                    move_card.run((card_name.get_value(), target));
                                                                }
                                                            }
                                                        >
                                                            <Icon name="ChevronRight" class="size-4" />
                                                        </Button>
                                                    </Show>
                                                </div>
                                            </Show>
                                        </div>
                                    </div>
                                }
                                .into_any()
                            })
                            .collect();
                        let is_over = {
                            let drop_label = drop_label.clone();
                            move || drag_over.get().as_deref() == Some(drop_label.as_str())
                        };
                        let on_dragover = {
                            let drop_label = drop_label.clone();
                            move |ev: leptos::ev::DragEvent| {
                                ev.prevent_default();
                                if movable && drag_over.get_untracked().as_deref() != Some(drop_label.as_str()) {
                                    drag_over.set(Some(drop_label.clone()));
                                }
                            }
                        };
                        let on_drop = {
                            let drop_label = drop_label.clone();
                            move |ev: leptos::ev::DragEvent| {
                                ev.prevent_default();
                                drag_over.set(None);
                                let name = ev
                                    .data_transfer()
                                    .and_then(|dt| dt.get_data("text/plain").ok())
                                    .filter(|name| !name.is_empty());
                                if let Some(name) = name {
                                    move_card.run((name, drop_label.clone()));
                                }
                            }
                        };
                        view! {
                            <div
                                class="w-72 shrink-0 snap-start transition-shadow duration-150 motion-reduce:transition-none sm:w-auto"
                                data-name="KanbanColumn"
                                data-column=drop_label.as_str()
                                on:dragover=on_dragover
                                on:dragleave=move |_| drag_over.set(None)
                                on:drop=on_drop
                            >
                                <Card class="flex flex-col gap-2 bg-muted/40 p-0">
                                    <div class="flex min-w-0 items-center justify-between gap-2 border-b px-3 py-2">
                                        <span class="truncate text-sm font-medium">{label.clone()}</span>
                                        <Badge variant=BadgeVariant::Secondary size=BadgeSize::Sm>{card_count}</Badge>
                                    </div>
                                    <div class=move || {
                                        if is_over() {
                                            "flex min-h-24 flex-col gap-2 rounded-md p-2 ring-2 ring-ring/50 ring-inset"
                                        } else {
                                            "flex min-h-24 flex-col gap-2 p-2"
                                        }
                                    }>
                                        {cards.into_iter().collect_view()}
                                        {(card_count == 0).then(|| view! {
                                            <p class="rounded-md border border-dashed border-border px-2 py-3 text-center text-xs text-muted-foreground">
                                                {i18n_view.t("views.common.no_documents")}
                                            </p>
                                        })}
                                    </div>
                                </Card>
                            </div>
                        }
                        .into_any()
                    })
                    .collect();
                view! {
                    <div class="flex snap-x snap-mandatory items-start gap-3 overflow-x-auto pb-2 sm:grid sm:grid-cols-2 sm:overflow-visible sm:pb-0 lg:grid-cols-3 xl:grid-cols-4">
                        {column_views.into_iter().collect_view()}
                    </div>
                }
                .into_any()
            }}
        </div>
    }
}

#[cfg(test)]
mod tests;
