//! Calendar view: records placed on a month (or week) grid by the doctype's
//! date field(s).
//!
//! The dates are metadata: `date_field` is the start (falling back to
//! `creation`, the one timestamp every document has) and `calendar_end_field`
//! — when declared — is the end, so a document that spans several days appears
//! on every day it covers. `calendar_title_field` names the card, and the
//! default is the doctype's title field.
//!
//! Navigation is real month/week paging (prev, next, today), not a list of
//! month cards: the grid always covers the full weeks the month touches, and a
//! document on a day is a link to its form. A day cell offers a create action
//! from that day (the form opens; the date is not prefilled), and the
//! empty/loading/error states say which one they are.
//!
//! The calendar opens on the current day. "Today" is resolved in the browser
//! (an effect), never during SSR, so the server render carries no date that the
//! hydrated render could disagree with.

mod model;

use chrono::{Datelike, Duration, NaiveDate, Utc};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map};
use serde_json::Value;

use crate::api::{self, DocStore, Filter, FilterOp, ListParams};
use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::card::{Card, CardContent};
use crate::components::ui::icon::Icon;
use crate::components::ui::select_native::SelectNative;
use crate::desk::session::path_segment;
use crate::views::views_components::collection::{
    stale_settings_message, unresolved_fields, use_collection_actions,
};
use crate::views::views_components::{
    EmptyState, Feedback, FeedbackKind, LoadingLines, PageHeader,
};
pub use model::{
    Scale, WEEKDAYS, event_range, grid_start, month_cells, month_start, parse_day, week_cells,
    week_start,
};
pub(crate) use model::{month_label, short_day, when_label};

/// How many documents one calendar page reads.
pub const PAGE: u64 = 500;

/// The `filters` JSON the list endpoint takes for one calendar window.
///
/// `start_field` is the declared `date_field` (or `creation`, the fallback the
/// grid renders with) and `end_field` the declared `calendar_end_field`, if any.
/// The window is filtered on the server instead of reading 500 arbitrary rows
/// and discarding most of them client-side. An event is visible when
///
/// ```text
/// start <= to  AND  (start >= from  OR  end >= from)
/// ```
///
/// and the wire grammar only joins *groups* with OR, so that is sent in
/// disjunctive normal form: `(start <= to AND start >= from) OR (start <= to
/// AND end >= from)`. A row with no `calendar_end_field` is single-day, so the
/// first conjunction is the whole test and the second is not emitted. No field
/// is invented: when the metadata declares no date field the caller passes the
/// documented `creation` fallback.
#[must_use]
pub fn range_filters(start_field: &str, end_field: Option<&str>, from: &str, to: &str) -> String {
    let upper = || Filter {
        field: start_field.to_string(),
        op: FilterOp::Lte,
        value: to.to_string(),
        or: false,
    };
    let lower = || Filter {
        field: start_field.to_string(),
        op: FilterOp::Gte,
        value: from.to_string(),
        or: false,
    };
    let mut filters = vec![upper(), lower()];
    if let Some(end_field) = end_field.filter(|field| !field.is_empty()) {
        // Second OR group: the same upper bound, joined with the end field.
        filters.push(Filter {
            or: true,
            ..upper()
        });
        filters.push(Filter {
            field: end_field.to_string(),
            op: FilterOp::Gte,
            value: from.to_string(),
            or: false,
        });
    }
    let rows: Vec<(String, String, String, bool)> = filters.iter().map(Filter::tuple).collect();
    serde_json::to_string(&rows).unwrap_or_default()
}

#[component]
pub fn CalendarView() -> impl IntoView {
    let params = use_params_map();
    let navigate = use_navigate();
    let go: Callback<String> = {
        let navigate = navigate.clone();
        Callback::new(move |path: String| {
            navigate(&path, leptos_router::NavigateOptions::default());
        })
    };
    // The route wins; `?doctype=` keeps an old bookmark working.
    let route_doctype =
        Signal::derive(move || params.with(|p| p.get("doctype").filter(|value| !value.is_empty())));
    let selected = RwSignal::new(String::new());
    Effect::new(move |_| {
        let from_route = route_doctype.get().unwrap_or_default();
        if selected.get_untracked() != from_route {
            selected.set(from_route);
        }
    });

    let meta = RwSignal::new(None::<crate::api::DynMeta>);
    let rows = RwSignal::new(Vec::<Value>::new());
    let total = RwSignal::new(0u64);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(None::<String>);
    let reload = RwSignal::new(0u64);
    // `None` until the browser mounts: SSR never paints a date it cannot know.
    let today = RwSignal::new(None::<NaiveDate>);
    let anchor = RwSignal::new(None::<NaiveDate>);
    let scale = RwSignal::new(Scale::Month);

    // The days the grid draws. Declared before the fetch effect because the
    // window it filters on is exactly this list: paging or switching scale is a
    // new range, not a client-side trim of the previous 500 rows.
    let cells = Memo::new(move |_| {
        let Some(anchor) = anchor.get() else {
            return Vec::new();
        };
        match scale.get() {
            Scale::Month => month_cells(anchor),
            Scale::Week => week_cells(anchor),
        }
    });

    Effect::new(move |_| {
        if today.get_untracked().is_none() {
            let now = Utc::now().date_naive();
            today.set(Some(now));
            anchor.set(Some(now));
        }
    });

    Effect::new(move |_| {
        let dt = selected.get();
        let _ = reload.get();
        if dt.is_empty() {
            rows.set(Vec::new());
            total.set(0);
            meta.set(None);
            loading.set(false);
            error.set(None);
            return;
        }
        loading.set(true);
        error.set(None);
        // The visible window: every day the grid draws, in the same order the
        // cells are rendered. Empty before the browser resolves "today", which
        // is the SSR pass — it asks for the doctype's first page as before.
        let days = cells.get();
        let window = days.first().copied().zip(days.last().copied());
        spawn_local(async move {
            let store = match DocStore::runtime() {
                Ok(store) => store,
                Err(message) => {
                    error.set(Some(message));
                    loading.set(false);
                    return;
                }
            };
            let found = match store.meta(&dt).await {
                Ok(found) => {
                    meta.set(found.clone());
                    found
                }
                Err(message) => {
                    error.set(Some(message));
                    loading.set(false);
                    return;
                }
            };
            // The range filter needs the fields the metadata declares; a
            // doctype with no `date_field` keeps the documented `creation`
            // fallback instead of inventing a field.
            let start_field = found
                .as_ref()
                .and_then(|meta| meta.date_field.clone())
                .filter(|field| !field.is_empty())
                .unwrap_or_else(|| "creation".to_string());
            let end_field = found
                .as_ref()
                .and_then(|meta| meta.calendar_end_field.clone())
                .filter(|field| !field.is_empty());
            let filters = window.map(|(from, to)| {
                range_filters(
                    &start_field,
                    end_field.as_deref(),
                    &from.format("%Y-%m-%d").to_string(),
                    &to.format("%Y-%m-%d").to_string(),
                )
            });
            let params = ListParams {
                limit_start: 0,
                limit_page_length: PAGE,
                filters,
                order_by: Some("creation asc".into()),
                fields: None,
                only_liked: None,
                search: None,
            };
            match store.list(&dt, &params).await {
                Ok(response) => {
                    total.set(response.total);
                    rows.set(response.data);
                }
                Err(message) => error.set(Some(message)),
            }
            loading.set(false);
        });
    });

    let on_reload = Callback::new(move |()| reload.update(|count| *count += 1));
    use_collection_actions(meta.read_only(), "calendar", on_reload);

    let stale_settings = Memo::new(move |_| {
        meta.get().as_ref().map_or_else(Vec::new, |meta| {
            unresolved_fields(
                meta,
                &[
                    ("Fecha", meta.date_field.as_deref()),
                    ("Fecha de fin", meta.calendar_end_field.as_deref()),
                    ("Título", meta.calendar_title_field.as_deref()),
                ],
            )
        })
    });

    // ── Metadata-derived configuration ──────────────────────────────────
    let declared_date = Memo::new(move |_| {
        meta.get()
            .as_ref()
            .and_then(|meta| meta.date_field.clone())
            .filter(|field| !field.is_empty())
    });
    let date_field = Memo::new(move |_| {
        declared_date
            .get()
            .unwrap_or_else(|| "creation".to_string())
    });
    let end_field = Memo::new(move |_| {
        meta.get()
            .as_ref()
            .and_then(|meta| meta.calendar_end_field.clone())
            .filter(|field| !field.is_empty())
    });
    let title_field = Memo::new(move |_| {
        meta.get().as_ref().map_or_else(
            || "name".to_string(),
            |meta| {
                meta.calendar_title_field
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
            .or_else(|| params.with(|p| p.get("module")))
            .unwrap_or_else(|| selected.get())
    });

    let heading = Memo::new(move |_| {
        let Some(anchor) = anchor.get() else {
            return String::new();
        };
        match scale.get() {
            Scale::Month => month_label(anchor),
            Scale::Week => {
                let start = week_start(anchor);
                let end = start + Duration::days(6);
                format!("{} – {}", short_day(start), short_day(end))
            }
        }
    });
    let fallback_date = Memo::new(move |_| declared_date.get().is_none());

    let step = Callback::new(move |forward: bool| {
        let Some(current) = anchor.get_untracked() else {
            return;
        };
        let days = match scale.get_untracked() {
            Scale::Month => 0,
            Scale::Week => 7,
        };
        let next = if days == 0 {
            let month = if forward {
                current
                    .checked_add_months(chrono::Months::new(1))
                    .unwrap_or(current)
            } else {
                current
                    .checked_sub_months(chrono::Months::new(1))
                    .unwrap_or(current)
            };
            month_start(month)
        } else if forward {
            current + Duration::days(days)
        } else {
            current - Duration::days(days)
        };
        anchor.set(Some(next));
    });

    // The picker is only needed where the route does not already name the
    // doctype (`/calendar`); on the canonical URL the switcher owns that job.
    let needs_picker = Signal::derive(move || route_doctype.get().is_none());
    let doctypes = crate::views::views_components::use_desk_doctypes();

    let pick = Callback::new(move |doctype: String| {
        selected.set(doctype.clone());
        if doctype.is_empty() {
            go.run("/calendar".to_string());
            return;
        }
        let module = meta
            .get_untracked()
            .map(|meta| meta.module)
            .filter(|module| !module.is_empty())
            .unwrap_or_else(|| doctype.clone());
        let path = format!(
            "/desk/{}/{}/calendar",
            path_segment(&module),
            path_segment(&doctype)
        );
        go.run(path);
    });

    let module_segment = move || path_segment(&module.get());
    let doctype_segment = move || path_segment(&selected.get());
    let new_url = Callback::new(move |()| {
        let url = format!("/desk/{}/{}/new", module_segment(), doctype_segment());
        go.run(url);
    });

    view! {
        <div class="mx-auto flex h-full min-h-0 w-full min-w-0 flex-col gap-4 p-4">
            {move || view! {
                <PageHeader
                    title="Calendario"
                    description=match scale.get() {
                        Scale::Month => "Documentos por día en una cuadrícula mensual.",
                        Scale::Week => "Documentos por día en una semana.",
                    }
                >
                    <Show when=move || needs_picker.get()>
                        <div class="w-full min-w-0 sm:w-auto">
                            <SelectNative
                                id="calendar-doctype"
                                class="w-full sm:w-56"
                                aria_label="DocType del calendario"
                                value=selected.read_only()
                                on_change=Callback::new(move |ev: leptos::ev::Event| {
                                    pick.run(event_target_value(&ev));
                                })
                            >
                                <option value="">"Elige un DocType…"</option>
                                {move || {
                                    let current = selected.get();
                                    let list = doctypes.get();
                                    let mut views: Vec<AnyView> = Vec::new();
                                    if !current.is_empty() && !list.contains(&current) {
                                        let label = current.clone();
                                        views.push(view! { <option value=current>{label}</option> }.into_any());
                                    }
                                    views.extend(list.into_iter().map(|name| {
                                        let label = name.clone();
                                        view! { <option value=name>{label}</option> }.into_any()
                                    }));
                                    views.into_iter().collect_view()
                                }}
                            </SelectNative>
                        </div>
                    </Show>
                    <div class="flex items-center gap-1">
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Icon
                            attr:aria-label="Periodo anterior"
                            on:click=move |_| step.run(false)
                        >
                            <Icon name="ChevronLeft" class="size-4" />
                        </Button>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            on:click=move |_| anchor.set(today.get_untracked())
                        >
                            "Hoy"
                        </Button>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Icon
                            attr:aria-label="Periodo siguiente"
                            on:click=move |_| step.run(true)
                        >
                            <Icon name="ChevronRight" class="size-4" />
                        </Button>
                        <div class="ms-1 flex items-center rounded-md border border-border p-0.5" role="group" aria-label="Escala del calendario">
                            <Button
                                variant=if scale.get() == Scale::Month { ButtonVariant::Secondary } else { ButtonVariant::Ghost }
                                size=ButtonSize::Sm
                                attr:aria-pressed=move || (scale.get() == Scale::Month).to_string()
                                on:click=move |_| scale.set(Scale::Month)
                            >
                                "Mes"
                            </Button>
                            <Button
                                variant=if scale.get() == Scale::Week { ButtonVariant::Secondary } else { ButtonVariant::Ghost }
                                size=ButtonSize::Sm
                                attr:aria-pressed=move || (scale.get() == Scale::Week).to_string()
                                on:click=move |_| scale.set(Scale::Week)
                            >
                                "Semana"
                            </Button>
                        </div>
                    </div>
                </PageHeader>
            }}

            <Feedback kind=FeedbackKind::Error message=Signal::derive(move || error.get()) />
            <Feedback
                kind=FeedbackKind::Warning
                message=Signal::derive(move || stale_settings_message(&stale_settings.get()))
            />

            <Show when=move || loading.get()>
                <Card><CardContent><LoadingLines lines=6 /></CardContent></Card>
            </Show>

            <Show when=move || !loading.get() && error.get().is_none() && selected.get().is_empty()>
                <EmptyState
                    icon="Calendar"
                    title="Elige un DocType"
                    description="El calendario muestra los documentos de un DocType con campo de fecha."
                />
            </Show>

            {move || {
                if loading.get() || error.get().is_some() || selected.get().is_empty() {
                    return AnyView::from(Fragment::new(Vec::new()));
                }
                let days = cells.get();
                if days.is_empty() {
                    return AnyView::from(Fragment::new(Vec::new()));
                }
                let start_field = date_field.get();
                let end_field = end_field.get();
                let end_field = end_field.as_deref();
                let title = title_field.get();
                let from = days.first().copied().unwrap_or_default();
                let to = days.last().copied().unwrap_or_default();
                let events: Vec<(NaiveDate, NaiveDate, Value)> = rows
                    .get()
                    .into_iter()
                    .filter_map(|row| {
                        event_range(&row, &start_field, end_field, from, to)
                            .map(|(start, end)| (start, end, row))
                    })
                    .collect();
                let by_day: std::collections::BTreeMap<NaiveDate, Vec<(String, String, String)>> = {
                    let mut map: std::collections::BTreeMap<NaiveDate, Vec<(String, String, String)>> =
                        std::collections::BTreeMap::new();
                    for (start, end, row) in &events {
                        let name = api::record_name(row);
                        let label = api::display_value(
                            "Data",
                            row.get(&title).unwrap_or(&Value::Null),
                        );
                        let label = if label.trim().is_empty() { name.clone() } else { label };
                        let mut day = *start;
                        while day <= *end {
                            map.entry(day).or_default().push((
                                name.clone(),
                                label.clone(),
                                when_label(row, &start_field),
                            ));
                            day += Duration::days(1);
                        }
                    }
                    map
                };
                if rows.with(Vec::is_empty) {
                    return view! {
                        <EmptyState
                            icon="Calendar"
                            title="Sin documentos"
                            description="Este DocType todavía no tiene documentos."
                        />
                    }.into_any();
                }
                // A period with no documents keeps its grid: hiding the days
                // would make paging look broken, and the note below says why
                // the cells are empty. Only a doctype with no documents at all
                // gets the full empty state above.
                let module = module_segment();
                let doctype = doctype_segment();
                let today = today.get();
                let is_month = scale.get() == Scale::Month;
                let total_shown = events.len();
                let period_total = total.get();
                let truncated =
                    period_total > u64::try_from(rows.with(Vec::len)).unwrap_or(u64::MAX);
                view! {
                    <div class="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                        <span class="text-sm font-medium text-foreground">{move || heading.get()}</span>
                        <Badge variant=BadgeVariant::Secondary size=BadgeSize::Sm>
                            {format!("{total_shown} de {period_total} documentos")}
                        </Badge>
                        {fallback_date.get().then(|| view! {
                            <span>"El DocType no declara campo de fecha: se usa «creation»."</span>
                        })}
                        {truncated.then(|| view! {
                            <Badge variant=BadgeVariant::Warning size=BadgeSize::Sm>
                                {format!("Mostrando los primeros {PAGE}")}
                            </Badge>
                        })}
                        {(total_shown == 0).then(|| view! {
                            <span>
                                "Ningún documento cae en el periodo mostrado: cambia de mes o revisa el campo de fecha."
                            </span>
                        })}
                    </div>

                    <Card class="min-h-0 flex-1 overflow-hidden p-0">
                        <CardContent class="flex h-full flex-col gap-0 p-0">
                            <div class="grid grid-cols-7 border-b border-border bg-muted/40">
                                {WEEKDAYS.into_iter().map(|day| view! {
                                    <div class="px-1 py-2 text-center text-[11px] font-medium text-muted-foreground">{day}</div>
                                }).collect_view()}
                            </div>
                            <div class="grid min-h-0 flex-1 auto-rows-fr grid-cols-7 overflow-y-auto">
                                {days.into_iter().map(|day| {
                                    let inside_month = !is_month || day.month() == month_start(anchor.get().unwrap_or(day)).month();
                                    let is_today = today == Some(day);
                                    let entries = by_day.get(&day).cloned().unwrap_or_default();
                                    let count = entries.len();
                                    let doctype = doctype.clone();
                                    let module = module.clone();
                                    let create = {
                                        view! {
                                            <Button
                                                variant=ButtonVariant::Ghost
                                                size=ButtonSize::Icon
                                                class="size-7 opacity-0 transition-opacity duration-150 group-hover:opacity-100 focus-visible:opacity-100 motion-reduce:transition-none"
                                                attr:aria-label="Crear documento"
                                                on:click=move |_| new_url.run(())
                                            >
                                                <Icon name="Plus" class="size-4" />
                                            </Button>
                                        }
                                    };
                                    view! {
                                        <div
                                            class=move || {
                                                let base = "group relative flex min-h-20 min-w-0 flex-col gap-1 border-b border-e border-border/60 p-1 text-xs transition-colors duration-150 motion-reduce:transition-none hover:bg-muted/30";
                                                let faded = if inside_month { "" } else { " opacity-50" };
                                                let today = if is_today { " ring-2 ring-inset ring-ring/60" } else { "" };
                                                format!("{base}{faded}{today}")
                                            }
                                            data-name="CalendarDay"
                                            data-date=day.format("%Y-%m-%d").to_string()
                                        >
                                            <div class="flex items-center justify-between gap-1">
                                                <span class=if is_today { "flex size-5 items-center justify-center rounded-full bg-primary text-[11px] font-semibold text-primary-foreground" } else { "text-[11px] font-medium text-muted-foreground" }>
                                                    {day.day().to_string()}
                                                </span>
                                                {create}
                                            </div>
                                            <div class="flex min-w-0 flex-col gap-0.5">
                                                {entries.into_iter().take(3).map(|(name, label, when)| {
                                                    let href = format!(
                                                        "/desk/{module}/{doctype}/{}",
                                                        path_segment(&name)
                                                    );
                                                    let label_text = label.clone();
                                                    let title_attr = format!("{label} · {when}");
                                                    view! {
                                                        <A
                                                            href=href
                                                            attr:class="min-w-0 truncate rounded-sm px-1 py-0.5 text-[11px] text-foreground hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
                                                            attr:title=title_attr
                                                        >
                                                            <span class="hidden sm:inline">{label_text}</span>
                                                            <span class="sm:hidden">"•"</span>
                                                        </A>
                                                    }
                                                }).collect_view()}
                                                {(count > 3).then(|| view! {
                                                    <span class="px-1 text-[10px] text-muted-foreground">
                                                        {format!("+{} más", count - 3)}
                                                    </span>
                                                })}
                                                {(count > 0 && count <= 3).then(|| view! {
                                                    <span class="px-1 text-[10px] text-muted-foreground sm:hidden">{count}</span>
                                                })}
                                            </div>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        </CardContent>
                    </Card>
                }.into_any()
            }}
        </div>
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod range_tests {
    use super::range_filters;
    use ferris_schema::contract::list::{FilterOp, or_groups};

    fn parse(json: &str) -> Vec<(String, FilterOp, String, bool)> {
        let rows: Vec<(String, String, String, bool)> =
            serde_json::from_str(json).expect("range filters are JSON");
        rows.into_iter()
            .map(|(field, op, value, or)| {
                (
                    field,
                    FilterOp::from_str(&op).expect("known operator"),
                    value,
                    or,
                )
            })
            .collect()
    }

    /// A single-day doctype is narrowed by one AND range on its date field.
    #[test]
    fn a_single_day_doctype_filters_one_field() {
        let rows = parse(&range_filters(
            "event_date",
            None,
            "2026-11-01",
            "2026-11-30",
        ));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "event_date");
        assert_eq!(rows[0].1, FilterOp::Lte);
        assert_eq!(rows[0].2, "2026-11-30");
        assert_eq!(rows[1].1, FilterOp::Gte);
        assert_eq!(rows[1].2, "2026-11-01");
        assert_eq!(
            or_groups(rows.iter().map(|row| row.3)),
            vec![vec![0, 1]],
            "the two bounds are ANDed, not ORed"
        );
    }

    /// The end field adds a second OR group, so a multi-day event that began
    /// before the window and still reaches it survives; the upper bound is
    /// repeated because groups are joined with OR.
    #[test]
    fn an_end_field_adds_a_conjunction_with_the_same_upper_bound() {
        let rows = parse(&range_filters(
            "starts_on",
            Some("ends_on"),
            "2026-11-01",
            "2026-11-30",
        ));
        assert_eq!(rows.len(), 4);
        let groups = or_groups(rows.iter().map(|row| row.3));
        assert_eq!(groups, vec![vec![0, 1], vec![2, 3]]);
        assert_eq!(rows[2].1, FilterOp::Lte);
        assert_eq!(rows[2].2, "2026-11-30");
        assert_eq!(rows[3].0, "ends_on");
        assert_eq!(rows[3].1, FilterOp::Gte);
        assert_eq!(rows[3].2, "2026-11-01");
    }

    /// An empty end field is the same as none: never emit a filter on a field
    /// the metadata did not declare.
    #[test]
    fn an_empty_end_field_is_ignored() {
        let rows = parse(&range_filters(
            "event_date",
            Some(""),
            "2026-11-01",
            "2026-11-30",
        ));
        assert_eq!(rows.len(), 2);
    }
}
