//! Gantt view: records drawn as bars between the doctype's start and end date
//! fields (`DoctypeMeta::gantt_start_field` / `gantt_end_field`).
//!
//! This is the **first functional version**, and it says what it does: a
//! horizontal day grid, one bar per document, the declared progress field
//! filling the bar, and the declared dependency shown as "after X" on the row.
//! It does **not** draw dependency arrows or support dragging bars — moving a
//! bar would be a date write, and pretending a drag happened is exactly the
//! silent success this codebase forbids. The configuration is metadata: a
//! doctype with no start/end fields gets an explanation and a link to its
//! settings, never a guessed timeline.
//!
//! The timeline is a fixed day grid rather than absolutely positioned bars
//! because the Desk forbids inline styles: a bar is the run of day cells that
//! fall inside its range, which keeps every width in a Tailwind class the
//! stylesheet actually ships.

use std::collections::BTreeSet;

use chrono::{Datelike, Duration, NaiveDate};
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;
use serde_json::Value;

use crate::api::{self, DocStore, ListParams};
use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::components::ui::card::{Card, CardContent};
use crate::components::ui::icon::Icon;
use crate::desk::session::path_segment;
use crate::views::views_components::collection::{
    stale_settings_message, unresolved_fields, use_collection_actions,
};
use crate::views::views_components::{
    EmptyState, Feedback, FeedbackKind, LoadingLines, PageHeader,
};

/// Rows read for the board. A Gantt over more documents is a report.
const PAGE: u64 = 500;
/// Days drawn at most: a year does not fit a screen and would be 365 cells per
/// row, so the window is the first part of the range and the view says so.
const MAX_DAYS: i64 = 120;

/// One document as a bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bar {
    pub name: String,
    pub label: String,
    /// Day offsets from the timeline origin.
    pub start: i64,
    pub end: i64,
    /// 0–100, or `None` when the doctype declares no progress field.
    pub progress: Option<i64>,
    /// The predecessor's title, when the dependency field points at one of the
    /// loaded rows.
    pub after: Option<String>,
}

/// The timeline the bars are drawn on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub origin: NaiveDate,
    pub days: i64,
    /// The range covered more than [`MAX_DAYS`] days and was cut.
    pub truncated: bool,
}

/// Parse the date part of a metadata value (`2026-11-21`, or the first ten
/// characters of an RFC3339 timestamp).
#[must_use]
pub fn parse_day(value: &Value) -> Option<NaiveDate> {
    let text = value.as_str()?;
    let head: String = text.chars().take(10).collect();
    NaiveDate::parse_from_str(&head, "%Y-%m-%d").ok()
}

/// The progress a row declares, clamped to 0–100.
///
/// A `Float` progress arrives as an `f64`; rounding it to a whole percent is
/// the only meaningful rendering, and the clamp above keeps the cast in range.
#[must_use]
#[allow(clippy::cast_possible_truncation)] // 0..=100 fits i64; see the clamp
pub fn progress_of(row: &Value, field: Option<&str>) -> Option<i64> {
    let field = field?;
    let value = row.get(field)?;
    let number = value
        .as_i64()
        .or_else(|| value.as_f64().map(|n| n.round() as i64))
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<f64>().ok())
                .map(|n| n.round() as i64)
        })?;
    Some(number.clamp(0, 100))
}

/// Build the bars and the window they need.
///
/// A row without a usable start is skipped: it cannot be placed, and inventing
/// today as its start would claim a date nobody set. An end before the start is
/// read as a single-day task instead of a negative bar.
#[must_use]
pub fn bars_and_timeline(
    rows: &[Value],
    start_field: &str,
    end_field: &str,
    title_field: &str,
    progress_field: Option<&str>,
    dependency_field: Option<&str>,
) -> (Vec<Bar>, Option<Timeline>) {
    let title_of = |row: &Value| {
        let name = api::record_name(row);
        let label = row
            .get(title_field)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .unwrap_or(&name);
        label.to_string()
    };
    let mut dated: Vec<(NaiveDate, NaiveDate, &Value)> = Vec::new();
    for row in rows {
        let Some(start) = row.get(start_field).and_then(parse_day) else {
            continue;
        };
        let end = row
            .get(end_field)
            .and_then(parse_day)
            .filter(|end| *end >= start)
            .unwrap_or(start);
        dated.push((start, end, row));
    }
    if dated.is_empty() {
        return (Vec::new(), None);
    }
    let min = dated
        .iter()
        .map(|(start, _, _)| *start)
        .min()
        .unwrap_or_default();
    let max = dated
        .iter()
        .map(|(_, end, _)| *end)
        .max()
        .unwrap_or_default();
    let span = (max - min).num_days().saturating_add(1);
    let days = span.clamp(1, MAX_DAYS);
    let titles: std::collections::BTreeMap<String, String> = dated
        .iter()
        .map(|(_, _, row)| (api::record_name(row), title_of(row)))
        .collect();

    let bars = dated
        .into_iter()
        .map(|(start, end, row)| {
            let name = api::record_name(row);
            let after = dependency_field
                .and_then(|field| row.get(field))
                .and_then(Value::as_str)
                .filter(|target| !target.is_empty())
                .map(|target| {
                    titles
                        .get(target)
                        .cloned()
                        .unwrap_or_else(|| target.to_string())
                });
            Bar {
                name,
                label: title_of(row),
                start: (start - min).num_days(),
                end: ((end - min).num_days()).min(days - 1),
                progress: progress_of(row, progress_field),
                after,
            }
        })
        .collect();

    (
        bars,
        Some(Timeline {
            origin: min,
            days,
            truncated: span > MAX_DAYS,
        }),
    )
}

/// The `set` of day indices a bar covers, clipped to the window.
#[must_use]
pub fn covered_days(bar: &Bar, days: i64) -> BTreeSet<i64> {
    let start = bar.start.max(0);
    let end = bar.end.min(days.saturating_sub(1));
    if end < start {
        return BTreeSet::new();
    }
    (start..=end).collect()
}

#[component]
pub fn GanttView() -> impl IntoView {
    let params = use_params_map();
    let doctype = Signal::derive(move || params.with(|p| p.get("doctype").unwrap_or_default()));

    let meta = RwSignal::new(None::<crate::api::DynMeta>);
    let rows = RwSignal::new(Vec::<Value>::new());
    let total = RwSignal::new(0u64);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(None::<String>);
    let reload = RwSignal::new(0u64);

    Effect::new(move |_| {
        let dt = doctype.get();
        let _ = reload.get();
        if dt.is_empty() {
            rows.set(Vec::new());
            total.set(0);
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
            match store.meta(&dt).await {
                Ok(found) => meta.set(found),
                Err(message) => {
                    error.set(Some(message));
                    loading.set(false);
                    return;
                }
            }
            let params = ListParams {
                limit_start: 0,
                limit_page_length: PAGE,
                filters: None,
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

    let on_reload = Callback::new(move |()| reload.update(|n| *n += 1));
    use_collection_actions(meta.read_only(), "gantt", on_reload);

    let stale_settings = Memo::new(move |_| {
        meta.get().as_ref().map_or_else(Vec::new, |meta| {
            unresolved_fields(
                meta,
                &[
                    ("Inicio", meta.gantt_start_field.as_deref()),
                    ("Fin", meta.gantt_end_field.as_deref()),
                    ("Dependencia", meta.gantt_dependency_field.as_deref()),
                    ("Progreso", meta.gantt_progress_field.as_deref()),
                ],
            )
        })
    });

    let start_field = Memo::new(move |_| {
        meta.get()
            .as_ref()
            .and_then(|meta| meta.gantt_start_field.clone())
            .filter(|field| !field.is_empty())
    });
    let end_field = Memo::new(move |_| {
        meta.get()
            .as_ref()
            .and_then(|meta| meta.gantt_end_field.clone())
            .filter(|field| !field.is_empty())
    });
    let progress_field = Memo::new(move |_| {
        meta.get()
            .as_ref()
            .and_then(|meta| meta.gantt_progress_field.clone())
    });
    let dependency_field = Memo::new(move |_| {
        meta.get()
            .as_ref()
            .and_then(|meta| meta.gantt_dependency_field.clone())
    });
    let title_field = Memo::new(move |_| {
        meta.get().as_ref().map_or_else(
            || "name".to_string(),
            |meta| api::title_field(meta).to_string(),
        )
    });
    let configured = Memo::new(move |_| start_field.get().is_some() && end_field.get().is_some());

    let model = Memo::new(move |_| {
        let (Some(start), Some(end)) = (start_field.get(), end_field.get()) else {
            return (Vec::new(), None);
        };
        bars_and_timeline(
            &rows.get(),
            &start,
            &end,
            &title_field.get(),
            progress_field.get().as_deref(),
            dependency_field.get().as_deref(),
        )
    });

    let module = Memo::new(move |_| {
        meta.get()
            .as_ref()
            .map(|meta| meta.module.clone())
            .filter(|module| !module.is_empty())
            .unwrap_or_else(|| params.with(|p| p.get("module").unwrap_or_default()))
    });

    let open_settings = move || {
        format!(
            "/desk/{}/{}/layout",
            path_segment(&module.get()),
            path_segment(&doctype.get())
        )
    };

    view! {
        <div class="mx-auto flex h-full min-h-0 w-full min-w-0 flex-col gap-4 p-4">
            {move || view! {
                <PageHeader
                    title=doctype.get()
                    description="Documentos como barras entre la fecha de inicio y la de fin."
                />
            }}

            <Feedback kind=FeedbackKind::Error message=Signal::derive(move || error.get()) />
            <Feedback
                kind=FeedbackKind::Warning
                message=Signal::derive(move || stale_settings_message(&stale_settings.get()))
            />

            <Show when=move || loading.get()>
                <Card>
                    <CardContent><LoadingLines lines=6 /></CardContent>
                </Card>
            </Show>

            <Show when=move || !loading.get() && error.get().is_none() && !configured.get()>
                <EmptyState
                    icon="ChartGantt"
                    title="Faltan las fechas del Gantt"
                    description="La vista de Gantt necesita un campo de inicio y uno de fin. Configúralos en «Ajustes del DocType» → Vista Gantt."
                >
                    <A
                        href=open_settings()
                        attr:class="inline-flex h-9 items-center gap-2 rounded-md bg-primary px-4 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                    >
                        <Icon name="Settings2" class="size-4" />
                        "Abrir ajustes del DocType"
                    </A>
                </EmptyState>
            </Show>

            <Show when=move || {
                !loading.get() && error.get().is_none() && configured.get() && model.get().1.is_none()
            }>
                <EmptyState
                    icon="ChartGantt"
                    title="Sin fechas"
                    description="Ningún documento tiene valores utilizables en los campos de inicio y fin."
                />
            </Show>

            {move || {
                let (bars, timeline) = model.get();
                let Some(timeline) = timeline else {
                    return AnyView::from(Fragment::new(Vec::new()));
                };
                if loading.get() || error.get().is_some() {
                    return AnyView::from(Fragment::new(Vec::new()));
                }
                let module = module.get();
                let dt = doctype.get();
                let doctype_segment = path_segment(&dt);
                let module_segment = path_segment(&module);
                let bar_count = bars.len();
                let page_total = total.get();
                let page_truncated = page_total > u64::try_from(rows.with(Vec::len)).unwrap_or(u64::MAX);
                let day_width = "w-7 min-w-7";
                let cells = |count: i64| (0..count).collect::<Vec<_>>();
                let header = view! {
                    <div class="flex items-end gap-2">
                        <div class="w-56 min-w-56 shrink-0 text-xs font-medium text-muted-foreground">"Documento"</div>
                        <div class="flex" aria-hidden="true">
                            {cells(timeline.days).into_iter().map(|index| {
                                let day = timeline.origin + Duration::days(index);
                                let month_start = day.day() == 1 || index == 0;
                                let label = if month_start {
                                    format!("{} {}", month_name(day.month()), day.day())
                                } else {
                                    day.day().to_string()
                                };
                                let weekday = weekday_name(day);
                                view! {
                                    <div class=format!("{day_width} flex flex-col items-center border-s border-border/60 px-0.5 text-[10px] text-muted-foreground")>
                                        <span class="truncate">{weekday}</span>
                                        <span class="truncate font-medium text-foreground">{label}</span>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    </div>
                };
                let body = bars.into_iter().map(|bar| {
                    let href = format!(
                        "/desk/{module_segment}/{doctype_segment}/{}",
                        path_segment(&bar.name)
                    );
                    let covered = covered_days(&bar, timeline.days);
                    let progress = bar.progress;
                    let total = bar.end.saturating_sub(bar.start).saturating_add(1).max(1);
                    let done = progress.map_or(0, |percent| (percent * total / 100).max(0));
                    let cells = cells(timeline.days).into_iter().map(|index| {
                        let inside = covered.contains(&index);
                        if !inside {
                            return view! { <div class=format!("{day_width} h-8 border-s border-border/30")></div> }.into_any();
                        }
                        let first = index == bar.start.max(0);
                        let last = index == bar.end.min(timeline.days - 1);
                        let class = if progress.is_some() {
                            if index - bar.start.max(0) < done {
                                "bg-primary"
                            } else {
                                "bg-primary/40"
                            }
                        } else {
                            "bg-primary"
                        };
                        let rounded = if first && last {
                            "rounded-md"
                        } else if first {
                            "rounded-s-md"
                        } else if last {
                            "rounded-e-md"
                        } else {
                            ""
                        };
                        view! {
                            <div class=format!("{day_width} h-8 border-s border-border/30 p-0.5")>
                                <div class=format!("h-full w-full {class} {rounded}")></div>
                            </div>
                        }
                        .into_any()
                    }).collect_view();
                    view! {
                        <div class="flex items-center gap-2 border-t border-border/40 py-1" data-name="GanttRow" data-document=bar.name>
                            <div class="flex w-56 min-w-56 shrink-0 flex-col gap-0.5">
                                <A
                                    href=href
                                    attr:class="min-w-0 truncate text-sm font-medium text-foreground hover:underline focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                                >
                                    {bar.label}
                                </A>
                                <div class="flex min-w-0 flex-wrap items-center gap-1">
                                    <span class="text-[11px] text-muted-foreground">
                                        {format!("{} → {}", timeline.origin + Duration::days(bar.start), timeline.origin + Duration::days(bar.end))}
                                    </span>
                                    {progress.map(|percent| view! {
                                        <Badge variant=BadgeVariant::Info size=BadgeSize::Sm>
                                            {format!("{percent}%")}
                                        </Badge>
                                    })}
                                    {bar.after.map(|target| view! {
                                        <span class="min-w-0 truncate text-[11px] text-muted-foreground">
                                            {format!("después de {target}")}
                                        </span>
                                    })}
                                </div>
                            </div>
                            <div class="flex">{cells}</div>
                        </div>
                    }
                }).collect_view();
                view! {
                    <div class="flex flex-col gap-2">
                        <div class="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                            <span>{format!("{bar_count} de {page_total} barras")}</span>
                            {page_truncated.then(|| view! {
                                <Badge variant=BadgeVariant::Warning size=BadgeSize::Sm>
                                    {format!("Mostrando los primeros {PAGE}")}
                                </Badge>
                            })}
                            <Show when=move || timeline.truncated>
                                <Badge variant=BadgeVariant::Warning size=BadgeSize::Sm>
                                    {format!("Ventana de {MAX_DAYS} días: las barras se recortan")}
                                </Badge>
                            </Show>
                        </div>
                        <Card class="p-0">
                            <CardContent class="overflow-x-auto p-2">
                                <div class="min-w-max">
                                    {header}
                                    {body}
                                </div>
                            </CardContent>
                        </Card>
                    </div>
                }
                .into_any()
            }}
        </div>
    }
}

const fn month_name(month: u32) -> &'static str {
    match month {
        1 => "ene",
        2 => "feb",
        3 => "mar",
        4 => "abr",
        5 => "may",
        6 => "jun",
        7 => "jul",
        8 => "ago",
        9 => "sep",
        10 => "oct",
        11 => "nov",
        12 => "dic",
        _ => "",
    }
}

fn weekday_name(day: NaiveDate) -> &'static str {
    match day.weekday() {
        chrono::Weekday::Mon => "L",
        chrono::Weekday::Tue => "M",
        chrono::Weekday::Wed => "X",
        chrono::Weekday::Thu => "J",
        chrono::Weekday::Fri => "V",
        chrono::Weekday::Sat => "S",
        chrono::Weekday::Sun => "D",
    }
}

#[cfg(test)]
mod tests;
