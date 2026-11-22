//! Report view: a doctype rendered as a table with filters, totals and export.
//!
//! ## The counters are the server's
//!
//! Borradores/Enviados/Cancelados used to be counted over the 500 rows the page
//! had loaded, so with more documents the three numbers described the first
//! page and not the report. They are now three `list` calls with
//! `limit_page_length=1` and a `docstatus` filter each, read from the server's
//! `total` — one row crosses the wire per counter, and the number is the truth
//! for the whole filtered set.
//!
//! The counters answer "how does the filtered set break down by state", so they
//! ignore a **status** filter the operator picked (otherwise all three would
//! show that one status); period and search filters do apply. The table below
//! honours every filter, including the status one.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use serde_json::Value;

use crate::api::types::filters_json;
use crate::api::{self, DocStore, Filter, FilterOp, ListParams};
use crate::components::ui::badge::{Badge, BadgeVariant};
use crate::components::ui::card::{Card, CardContent, CardDescription, CardHeader, CardTitle};
use crate::components::ui::table as t;
use crate::views::views_components::collection::use_collection_actions;
use crate::views::views_components::{
    Feedback, FeedbackKind, LoadingLines, PageHeader, ReportFilters, StatCard, StatCardView,
};

/// Per-state counts read from the server: `(draft, submitted, cancelled)`.
type StatusCounts = (u64, u64, u64);

#[component]
pub fn ReportView() -> impl IntoView {
    let params = use_params_map();
    // Two routes reach this page: the canonical
    // `/desk/:module/:doctype/report` (doctype param) and the legacy
    // `/report/:name` (name param). The report is a doctype report, so the
    // doctype segment is what it renders.
    let doctype = move || {
        params.with(|p| {
            p.get("doctype")
                .filter(|value| !value.is_empty())
                .or_else(|| p.get("name"))
                .unwrap_or_default()
        })
    };

    let meta = RwSignal::new(None::<crate::api::DynMeta>);
    let rows = RwSignal::new(Vec::<Value>::new());
    let total = RwSignal::new(0u64);
    let counts = RwSignal::new(StatusCounts::default());
    let (loading, set_loading) = signal(true);
    let (error, set_error) = signal(None::<String>);
    let filters = RwSignal::new(Vec::<crate::api::Filter>::new());
    // Bumped by the "Ejecutar" button. The fetch follows this counter, not the
    // filter signal: the button writes the filters *and* asks for a run, and
    // following both used to fire two identical requests (with two sets of
    // status counters) for one click.
    let run_token = RwSignal::new(0u64);

    let run = move || {
        let dt = doctype();
        if dt.is_empty() {
            return;
        }
        set_loading.set(true);
        set_error.set(None);
        // Read untracked: this closure is run by the effect below, which must
        // depend on `run_token`, never on the filters it passes through.
        let fs = filters.get_untracked();
        let listed = ListParams {
            limit_start: 0,
            limit_page_length: 500,
            filters: Some(filters_json(&fs)),
            order_by: Some("creation desc".into()),
            fields: None,
            only_liked: None,
            search: None,
        };
        api::drive(async move {
            let store = match DocStore::runtime() {
                Ok(store) => store,
                Err(message) => {
                    set_error.set(Some(message));
                    set_loading.set(false);
                    return;
                }
            };
            let submittable = match store.meta(&dt).await {
                Ok(found) => {
                    let submittable = found.as_ref().is_some_and(|meta| meta.is_submittable);
                    meta.set(found);
                    submittable
                }
                Err(message) => {
                    set_error.set(Some(message));
                    false
                }
            };
            match store.list(&dt, &listed).await {
                Ok(response) => {
                    total.set(response.total);
                    // The server already applied `filters`; re-filtering the
                    // page here would be a second, independently updated copy
                    // of the same policy.
                    rows.set(response.data);
                }
                Err(message) => set_error.set(Some(message)),
            }
            if submittable {
                match status_counts(&store, &dt, &fs).await {
                    Ok(read) => counts.set(read),
                    Err(message) => set_error.set(Some(message)),
                }
            } else {
                // `docstatus` is a lifecycle column: a doctype without a
                // submit/cancel flow has nothing to count, and drawing three
                // zeroes would invent a workflow.
                counts.set(StatusCounts::default());
            }
            set_loading.set(false);
        });
    };

    Effect::new(move |_| {
        let _ = run_token.get();
        run();
    });

    // Shared collection chrome: the view switcher moves to the list/board, and
    // refresh re-runs the report with the filters on screen.
    let on_reload = Callback::new(move |()| run_token.update(|token| *token += 1));
    use_collection_actions(meta.read_only(), "report", on_reload);

    // Derived once per metadata change: `list_columns` clones every column it
    // builds, and the table used to recompute the whole set once per row.
    let ordered = Memo::new(move |_| {
        meta.get()
            .as_ref()
            .map(api::list_columns)
            .unwrap_or_default()
    });
    let total_rows = move || total.get().to_string();
    let drafts = move || counts.get().0.to_string();
    let submitted = move || counts.get().1.to_string();
    let cancelled = move || counts.get().2.to_string();
    let submittable = move || meta.get().as_ref().is_some_and(|m| m.is_submittable);

    view! {
        <div class="flex h-full flex-col gap-4 overflow-y-auto p-4">
            {move || view! {
                <PageHeader
                    title=doctype()
                    description="Informe del tipo de documento con filtros, totales y exportación."
                />
            }}
            <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
                <StatCardView stat=StatCard { label: "Total".into(), value: Signal::derive(total_rows), change: None, icon: "Rows3".into() } />
                <Show when=submittable>
                    <StatCardView stat=StatCard { label: "Borradores".into(), value: Signal::derive(drafts), change: None, icon: "FileText".into() } />
                </Show>
                <Show when=submittable>
                    <StatCardView stat=StatCard { label: "Enviados".into(), value: Signal::derive(submitted), change: None, icon: "CircleCheck".into() } />
                </Show>
                <Show when=submittable>
                    <StatCardView stat=StatCard { label: "Cancelados".into(), value: Signal::derive(cancelled), change: None, icon: "CircleX".into() } />
                </Show>
            </div>
            <Show when=submittable>
                <p class="text-xs text-muted-foreground">
                    "El reparto por estado cuenta el conjunto filtrado, sin aplicar el filtro de estado de la tabla."
                </p>
            </Show>

            <Card>
                <CardHeader>
                    <CardTitle>"Resumen"</CardTitle>
                    <CardDescription>"Filas del conjunto filtrado y su flujo de estado."</CardDescription>
                </CardHeader>
                <CardContent class="flex flex-row flex-wrap items-center gap-2">
                    <Badge variant=BadgeVariant::Secondary>
                        {move || format!("{} filas", total.get())}
                    </Badge>
                    <Show when=move || { total.get() > 500 }>
                        <Badge variant=BadgeVariant::Warning>"Mostrando las primeras 500"</Badge>
                    </Show>
                    <Show when=submittable>
                        <Badge variant=BadgeVariant::Info>"Con flujo de estado"</Badge>
                    </Show>
                </CardContent>
            </Card>

            <ReportFilters
                filters=filters
                on_run=move || run_token.update(|token| *token += 1)
                on_export=move || rows.with(|r| api::export_csv(&doctype(), &ordered.get(), r))
            />

            <Feedback kind=FeedbackKind::Error message=Signal::derive(move || error.get()) />

            <Show when=move || loading.get()>
                <Card>
                    <CardContent>
                        <LoadingLines lines=4 />
                    </CardContent>
                </Card>
            </Show>
            <Show when=move || !loading.get() && error.get().is_none()>
                <Card>
                    // The table keeps a readable column width and scrolls
                    // inside the card instead of squeezing every column into a
                    // phone-sized strip.
                    <CardContent class="p-0 overflow-x-auto">
                        <t::Table class="min-w-[560px] [&_th:first-child]:pl-4 [&_td:first-child]:pl-4 sm:[&_th:first-child]:pl-6 sm:[&_td:first-child]:pl-6 [&_th:last-child]:pr-4 [&_td:last-child]:pr-4 sm:[&_th:last-child]:pr-6 sm:[&_td:last-child]:pr-6">
                            <t::TableHeader class="bg-muted/40">
                                <t::TableRow>
                                    {
                                        move || ordered.get().iter().map(|c| {
                                            let label = c.label.clone();
                                            view! { <t::TableHead>{label}</t::TableHead> }
                                        }).collect_view()
                                    }
                                </t::TableRow>
                            </t::TableHeader>
                            <t::TableBody>
                                {
                                    move || {
                                        let span = ordered.get().len().max(1);
                                        rows.with(|rows| {
                                            if rows.is_empty() {
                                                return view! {
                                                    <t::TableRow>
                                                        <td colspan=span.to_string() class="p-8 text-center text-muted-foreground">"Sin resultados"</td>
                                                    </t::TableRow>
                                                }
                                                    .into_any();
                                            }
                                            let cols = ordered.get();
                                            rows.iter().map(|row| {
                                                // Cells are rendered eagerly so the
                                                // row owns its views: nothing here
                                                // may borrow the row or the columns.
                                                let cells: Vec<AnyView> = cols
                                                    .iter()
                                                    .map(|c| {
                                                        let val = api::display_value(
                                                            &c.fieldtype,
                                                            row.get(&c.fieldname).unwrap_or(&Value::Null),
                                                        );
                                                        view! {
                                                            <t::TableCell class="max-w-sm truncate">
                                                                {val}
                                                            </t::TableCell>
                                                        }
                                                            .into_any()
                                                    })
                                                    .collect();
                                                view! { <t::TableRow>{cells.into_iter().collect_view()}</t::TableRow> }
                                            }).collect_view().into_any()
                                        })
                                    }
                                }
                            </t::TableBody>
                        </t::Table>
                    </CardContent>
                </Card>
            </Show>
        </div>
    }
}

/// One `list` per status, `limit_page_length=1`, reading the server's `total`.
///
/// # Errors
///
/// The first storage/transport message; a counter that could not be read is not
/// rendered as zero (that would claim there are no drafts when the truth is
/// that nobody knows).
async fn status_counts(
    store: &DocStore,
    doctype: &str,
    base: &[Filter],
) -> Result<StatusCounts, String> {
    let mut out = StatusCounts::default();
    for status in 0..3_u8 {
        let params = ListParams {
            limit_start: 0,
            // One row: the counter comes from `total`, not from the page.
            limit_page_length: 1,
            filters: Some(filters_json(&with_status(base, status))),
            order_by: None,
            fields: Some(vec!["name".to_string()]),
            only_liked: None,
            search: None,
        };
        let response = store.list(doctype, &params).await?;
        let slot = match status {
            0 => &mut out.0,
            1 => &mut out.1,
            _ => &mut out.2,
        };
        *slot = response.total;
    }
    Ok(out)
}

/// `base` without any `docstatus` filter, plus `docstatus = status`.
///
/// The operator's status filter is stripped: the three counters *are* the
/// status split, so keeping it would make all three report the same number.
fn with_status(base: &[Filter], status: u8) -> Vec<Filter> {
    let mut filters: Vec<Filter> = base
        .iter()
        .filter(|filter| filter.field != "docstatus")
        .cloned()
        .collect();
    filters.push(Filter {
        field: "docstatus".to_string(),
        op: FilterOp::Equals,
        // The document stores the word, not the column's smallint: a filter
        // built from the rank (`"0"`) matched no row at all, which is why the
        // three counters used to read zero over a non-empty table.
        value: docstatus_word(status).to_string(),
        or: false,
    });
    filters
}

/// The word the document's `docstatus` carries for a rank (`0/1/2`).
const fn docstatus_word(status: u8) -> &'static str {
    match status {
        1 => "submitted",
        2 => "cancelled",
        _ => "draft",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(field: &str, value: &str) -> Filter {
        Filter {
            field: field.to_string(),
            op: FilterOp::Equals,
            value: value.to_string(),
            or: false,
        }
    }

    /// A status the operator picked describes the *table*, not the split: the
    /// three counters must not all become that one status.
    #[test]
    fn the_status_split_replaces_a_picked_status_filter() {
        let base = vec![filter("docstatus", "1"), filter("creation", "7d")];
        let drafts = with_status(&base, 0);
        assert_eq!(drafts.len(), 2);
        assert!(
            drafts
                .iter()
                .any(|f| f.field == "creation" && f.value == "7d")
        );
        let status: Vec<&Filter> = drafts.iter().filter(|f| f.field == "docstatus").collect();
        assert_eq!(status.len(), 1);
        assert_eq!(status[0].value, "draft");
    }

    /// The document stores the word; the smallint is only the table column.
    #[test]
    fn the_counter_speaks_the_document_word() {
        assert_eq!(docstatus_word(0), "draft");
        assert_eq!(docstatus_word(1), "submitted");
        assert_eq!(docstatus_word(2), "cancelled");
    }

    #[test]
    fn unrelated_filters_survive_every_counter() {
        let base = vec![filter("name", "%INV%")];
        for status in 0..3 {
            let filters = with_status(&base, status);
            assert!(filters.iter().any(|f| f.field == "name"));
            assert!(filters.iter().any(|f| f.field == "docstatus"));
        }
    }

    /// The serialized filter is what the server parses: a 4-tuple per row with
    /// the `or` flag in the last position.
    #[test]
    fn the_counter_filters_serialize_in_the_engine_shape() {
        let json = filters_json(&with_status(&[filter("creation", "today")], 2));
        assert_eq!(
            json,
            r#"[["creation","equals","today",false],["docstatus","equals","cancelled",false]]"#
        );
    }
}
