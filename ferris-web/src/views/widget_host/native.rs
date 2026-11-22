//! The native renderers behind a `host.*` definition: stat, trend and table.

use std::collections::BTreeMap;

use leptos::prelude::*;
use leptos_router::components::A;
use serde_json::Value;

use crate::api::{self, DocStore, ListParams};
use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::components::ui::charts::{AreaChart, ChartCurve};
use crate::components::ui::table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow};
use crate::ext::WidgetDef;
use crate::views::views_components::{Feedback, FeedbackKind, LoadingLines};

use super::params::{number_param, param_or};
use super::placeholder::{missing_param, unknown_host_widget};

/// A native renderer for a `host.*` definition (Fase 2: the parametrized
/// replacements for the hardcoded `/` landing and `/dashboard/:name` views).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum HostRenderer {
    Stat,
    Trend,
    Table,
}

/// The `host.*` name → renderer table [`host_widget`] dispatches on. The
/// desk's half of the coverage test against
/// [`ferris_core::ui_extensions::host_widget_defs`] (the `HOST_WIDGETS`
/// catalog): a name outside this table degrades to
/// [`unknown_host_widget`], and the test fails before a catalog entry can
/// grow one of those in production.
pub(super) fn native_renderer(name: &str) -> Option<HostRenderer> {
    match name {
        "stat" => Some(HostRenderer::Stat),
        "trend" => Some(HostRenderer::Trend),
        "table" => Some(HostRenderer::Table),
        _ => None,
    }
}

/// Native body for a `host.*` definition: the data the hardcoded dashboards
/// used to fetch, asked for by the instance's params instead of baked into
/// a view.
///
/// The body is content only: no `Card`, no title, no subtitle. The tile is the
/// card and [`super::host::WidgetHost`] paints the heading; keeping those out
/// of here is what lets `show_title`/`subtitle` work the same for native and
/// plugin widgets.
pub(super) fn host_widget(
    def: &WidgetDef,
    params: &[(String, String)],
    refresh: Option<Signal<u32>>,
) -> AnyView {
    // `doctype` is `required` in the host catalog; without it the list call
    // can only fail, so say what is missing instead.
    let Some(doctype) = param_or(def, params, "doctype").filter(|value| !value.is_empty()) else {
        return missing_param(def, "doctype").into_any();
    };
    let doctype = doctype.to_string();

    // Exhaustive over `Option<HostRenderer>` on purpose — no wildcard arm
    // to fall through to, so a new renderer variant forces its body here
    // at compile time.
    match native_renderer(&def.name) {
        Some(HostRenderer::Stat) => view! {
            <HostStat
                doctype=doctype
                docstatus=param_or(def, params, "docstatus").unwrap_or("all").to_string()
                refresh=refresh
            />
        }
        .into_any(),
        Some(HostRenderer::Trend) => view! {
            <HostTrend doctype=doctype days=number_param(def, params, "days", 30) refresh=refresh />
        }
        .into_any(),
        Some(HostRenderer::Table) => view! {
            <HostTable
                doctype=doctype
                limit=number_param(def, params, "limit", 10)
                order=param_or(def, params, "order").unwrap_or("desc").to_string()
                refresh=refresh
            />
        }
        .into_any(),
        // The host catalog grew a name these renderers predate: a framed
        // body that says so, the same degradation a plugin's unknown kind
        // gets on the other side of the fence.
        None => unknown_host_widget(def).into_any(),
    }
}

/// `docstatus` param → the structural filter row the list engine parses;
/// `"all"` (the catalog's default) means no filter at all.
fn docstatus_filter(docstatus: &str) -> Option<String> {
    match docstatus {
        "all" | "" => None,
        value => Some(
            serde_json::to_string(&[("docstatus", "equals", value, false)]).unwrap_or_default(),
        ),
    }
}

/// `host.stat`: how many documents a DocType has (optionally restricted to
/// one docstatus), as the landing dashboard's KPI card.
#[component]
fn HostStat(
    doctype: String,
    docstatus: String,
    #[prop(optional_no_strip)] refresh: Option<Signal<u32>>,
) -> impl IntoView {
    let total = RwSignal::new(0_u64);
    let loading = RwSignal::new(true);

    // Effects run post-hydration: SSR renders the card shell with "…"; the
    // browser asks the DocStore as the session user. Reading `refresh` inside
    // the effect subscribes to the dashboard's timer, so a live dashboard
    // re-fetches without a remount.
    Effect::new(move |_| {
        if let Some(refresh) = refresh {
            refresh.get();
        }
        let doctype = doctype.clone();
        let docstatus = docstatus.clone();
        api::drive(async move {
            if let Ok(store) = DocStore::runtime() {
                let params = ListParams {
                    limit_start: 0,
                    limit_page_length: 1,
                    filters: docstatus_filter(&docstatus),
                    ..Default::default()
                };
                if let Ok(response) = store.list(&doctype, &params).await {
                    total.set(response.total);
                }
            }
            loading.set(false);
        });
    });

    let value = Signal::derive(move || {
        if loading.get() {
            "…".to_string()
        } else {
            total.get().to_string()
        }
    });

    view! {
        // The value is the body: the label is the host header, not this card.
        <div class="flex h-full items-center justify-between gap-3">
            <span class="text-2xl font-semibold">{move || value.get()}</span>
            <span class="text-2xl" aria-hidden="true">"📊"</span>
        </div>
    }
}

/// `host.trend`: documents created per day over the last `days` days, as
/// the landing dashboard's activity chart.
///
/// The counts come from the newest page the list endpoint returns (the engine
/// has no `GROUP BY` on this path), so the chart is labelled "Documentos" and
/// says nothing about a period it cannot guarantee. A failed read is reported
/// instead of drawn: an empty chart already means "no documents", and a silent
/// failure would read as one.
#[component]
fn HostTrend(
    doctype: String,
    days: u64,
    #[prop(optional_no_strip)] refresh: Option<Signal<u32>>,
) -> impl IntoView {
    let trend = RwSignal::new(Vec::<(String, u64)>::new());
    let (loading, set_loading) = signal(true);
    let (error, set_error) = signal(None::<String>);

    Effect::new(move |_| {
        if let Some(refresh) = refresh {
            refresh.get();
        }
        let doctype = doctype.clone();
        set_loading.set(true);
        set_error.set(None);
        api::drive(async move {
            let store = match DocStore::runtime() {
                Ok(store) => store,
                Err(message) => {
                    set_error.set(Some(message));
                    set_loading.set(false);
                    return;
                }
            };
            let params = ListParams {
                limit_start: 0,
                // The newest page is plenty for a bounded number of days.
                limit_page_length: 500,
                order_by: Some("creation desc".into()),
                // Only the day is counted; asking for the whole document would
                // move every blob over the wire for a histogram.
                fields: Some(vec!["creation".to_string()]),
                ..Default::default()
            };
            match store.list(&doctype, &params).await {
                Ok(response) => {
                    let mut by_day = BTreeMap::<String, u64>::new();
                    for row in &response.data {
                        if let Some(Value::String(created)) = row.get("creation") {
                            let day: String = created.chars().take(10).collect();
                            *by_day.entry(day).or_insert(0) += 1;
                        }
                    }
                    let mut points: Vec<(String, u64)> = by_day.into_iter().collect();
                    // Buckets are ascending: keep the newest `days` of them.
                    if points.len() as u64 > days {
                        let days = usize::try_from(days).unwrap_or(usize::MAX);
                        points.drain(..points.len().saturating_sub(days));
                    }
                    trend.set(points);
                }
                Err(message) => set_error.set(Some(message)),
            }
            set_loading.set(false);
        });
    });

    let labels = Signal::derive(move || {
        let dates: Vec<String> = trend.get().iter().map(|(day, _)| day.clone()).collect();
        serde_json::to_string(&dates).unwrap_or_else(|_| "[]".into())
    });
    let values = Signal::derive(move || {
        let counts: Vec<u64> = trend.get().iter().map(|(_, count)| *count).collect();
        serde_json::to_string(&counts).unwrap_or_else(|_| "[]".into())
    });

    view! {
        // The chart is the body; the host header owns the title/subtitle.
        <div class="flex h-full min-h-32 flex-col gap-1">
            <Show when=move || loading.get()>
                <LoadingLines lines=2 />
            </Show>
            <Show when=move || !loading.get() && error.get().is_some()>
                <Feedback
                    kind=FeedbackKind::Error
                    message=Signal::derive(move || error.get())
                />
            </Show>
            <Show when=move || !loading.get() && error.get().is_none()>
                <div class="min-h-24 flex-1">
                    <AreaChart
                        json_values=values
                        json_labels=labels
                        curve=ChartCurve::Smooth
                        class=Signal::derive(|| "h-full".to_string())
                        series_names="Documentos"
                        gradient=true
                        show_grid=true
                        show_yaxis=true
                    />
                </div>
            </Show>
        </div>
    }
}

/// `host.table`: the DocType's most recent documents.
#[component]
fn HostTable(
    doctype: String,
    limit: u64,
    order: String,
    #[prop(optional_no_strip)] refresh: Option<Signal<u32>>,
) -> impl IntoView {
    let rows = RwSignal::new(Vec::<Value>::new());
    let loading = RwSignal::new(true);
    let link_doctype = doctype.clone();

    Effect::new(move |_| {
        if let Some(refresh) = refresh {
            refresh.get();
        }
        let doctype = doctype.clone();
        let order = order.clone();
        api::drive(async move {
            if let Ok(store) = DocStore::runtime() {
                let params = ListParams {
                    limit_start: 0,
                    limit_page_length: limit,
                    order_by: Some(format!("creation {order}")),
                    ..Default::default()
                };
                if let Ok(response) = store.list(&doctype, &params).await {
                    rows.set(response.data);
                }
            }
            loading.set(false);
        });
    });

    view! {
        // Table body only: the tile is the card and `WidgetHost` paints the
        // heading, so no `Card`/`CardHeader` here.
        <div class="h-full overflow-auto">
            <Table class="min-w-[280px]">
                    <TableHeader class="bg-muted/40">
                        <TableRow>
                            <TableHead>"Nombre"</TableHead>
                            <TableHead>"Creado"</TableHead>
                            <TableHead>"Estado"</TableHead>
                        </TableRow>
                    </TableHeader>
                    <TableBody>
                        {move || {
                            let current = rows.get();
                            if current.is_empty() {
                                let message = if loading.get() { "Cargando…" } else { "Sin documentos." };
                                return view! {
                                    <TableRow>
                                        <TableCell
                                            attr:colspan="3"
                                            class="text-muted-foreground text-xs"
                                        >
                                            {message.to_string()}
                                        </TableCell>
                                    </TableRow>
                                }
                                .into_any();
                            }
                            current
                                .into_iter()
                                .map(|row| {
                                    let name = api::record_name(&row);
                                    // The cell shows a date, not a timestamp: a
                                    // raw `2026-11-15T10:00:00Z` does not fit a
                                    // widget tile and reads as machine output.
                                    let created: String = row
                                        .get("creation")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .chars()
                                        .take(10)
                                        .collect();
                                    let ds = row.get("docstatus").map(api::docstatus_rank).unwrap_or(0);
                                    let (status, status_variant) = match ds {
                                        1 => ("Enviado", BadgeVariant::Success),
                                        2 => ("Cancelado", BadgeVariant::Destructive),
                                        _ => ("Borrador", BadgeVariant::Secondary),
                                    };
                                    let href = format!(
                                        "/desk/{}/{}/{}",
                                        crate::desk::session::path_segment(&link_doctype),
                                        crate::desk::session::path_segment(&link_doctype),
                                        crate::desk::session::path_segment(&name)
                                    );
                                    view! {
                                        <TableRow>
                                            <TableCell class="font-medium">
                                                <A
                                                    href=href
                                                    attr:class="hover:underline"
                                                >{name.clone()}</A>
                                            </TableCell>
                                            <TableCell class="text-muted-foreground text-xs">{created}</TableCell>
                                            <TableCell>
                                                <Badge variant=status_variant size=BadgeSize::Sm>{status}</Badge>
                                            </TableCell>
                                        </TableRow>
                                    }
                                })
                                .collect_view()
                                .into_any()
                        }}
                    </TableBody>
                </Table>
        </div>
    }
}
