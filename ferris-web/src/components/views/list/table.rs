//! List view table component.
//!
//! The table body is virtualized: only the rows around the viewport are built,
//! and scrolling near the end asks the core for the next append. The header is
//! sticky and stays outside the virtual window.

mod empty;
mod row;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;
use serde_json::Value;
use wasm_bindgen::JsCast;

use crate::api::DynMeta;
use crate::api::meta::{Column, record_name, title_field};
use crate::api::types::EngagementPage;
use crate::components::constants::pagination::PAGINATION;
use crate::components::hooks::use_virtual_scroll::use_virtual_scroll;
use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::icon::Icon;
use crate::components::ui::spinner::Spinner;
use crate::components::ui::table as t;
use crate::ext::{SLOT_LIST_ROW_ACTIONS, UiCtx};
use empty::ListEmptyBlock;
use row::ListRow;

/// How close to the end of the loaded rows the viewport has to get before the
/// next page is requested. A few rows of lookahead keeps the append invisible.
const LOAD_MORE_THRESHOLD: usize = 4;

/// Compact age in Spanish list shorthand: `"ahora"`, `"2m"`, `"3h"`, `"3d"`,
/// `"2me"`. `elapsed` is seconds; negatives are clamped by the caller.
fn format_age(elapsed: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const MONTH: i64 = 30 * DAY;

    let (step, unit) = match elapsed {
        s if s < MINUTE => return "ahora".into(),
        s if s < HOUR => (MINUTE, "m"),
        s if s < DAY => (HOUR, "h"),
        s if s < MONTH => (DAY, "d"),
        _ => (MONTH, "me"),
    };
    format!("{}{unit}", elapsed / step)
}

/// Age of a document's timestamp plus the exact date for the tooltip.
/// Em-dash when the stamp is missing or unparseable, so a row without one
/// still lines up with the rest.
fn stamp_age(field: &str, row: &Value) -> (String, String) {
    let Some(ts) = row
        .get(field)
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    else {
        return ("—".into(), String::new());
    };

    let ts = ts.with_timezone(&chrono::Utc);
    let elapsed = (chrono::Utc::now() - ts).num_seconds().max(0);
    (format_age(elapsed), ts.format("%d/%m/%Y %H:%M").to_string())
}

/// Whether a row click landed on a control rather than on the row surface.
///
/// The whole row opens the document, but the checkbox, the title link, the
/// like button and a plugin's row actions keep their own behaviour. Resolving
/// that by the event target — instead of wrapping each control in a
/// `stop_propagation` handler — also covers the controls a plugin mounts in
/// the actions cell.
fn interactive_target(target: Option<web_sys::EventTarget>) -> bool {
    let Some(target) = target else {
        return true;
    };
    let Ok(element) = target.dyn_into::<web_sys::Element>() else {
        return true;
    };
    element
        .closest("a, button, input, select, textarea, label, [data-row-action]")
        .ok()
        .flatten()
        .is_some()
}

/// Whether the rendered window has reached the point where the next page should
/// be requested.
///
/// Kept pure so the scroll trigger can be tested without a DOM: the interesting
/// cases are the last rows, the already-loading state and the exhausted list.
fn should_load_more(end_index: usize, loaded: usize, has_more: bool, loading_more: bool) -> bool {
    has_more
        && !loading_more
        && loaded > 0
        && end_index.saturating_add(LOAD_MORE_THRESHOLD) >= loaded
}

#[component]
pub fn ListTable(
    #[prop(into)] visible_columns: Signal<Vec<Column>>,
    #[prop(into)] rows: Signal<Vec<Value>>,
    #[prop(into)] selected: Signal<HashSet<String>>,
    /// Documents matching the query, across every page: the header shows it
    /// next to the rows actually loaded.
    total: ReadSignal<u64>,
    /// Like and comment counts for the loaded rows.
    #[prop(into)]
    engagement: Signal<EngagementPage>,
    /// Whether the "only my likes" filter is on.
    #[prop(into)]
    only_liked: Signal<bool>,
    /// A like write is in flight, so the hearts can show as busy.
    #[prop(into)]
    pending_like: Signal<bool>,
    #[prop(into)] sort_state: Signal<(String, bool)>,
    #[prop(into)] meta: Signal<Option<DynMeta>>,
    /// A next page is in flight.
    #[prop(into)]
    loading_more: Signal<bool>,
    /// There are still rows after the loaded prefix.
    #[prop(into)]
    has_more: Signal<bool>,
    on_sort: Callback<String>,
    on_select_all: Callback<bool>,
    on_select_row: Callback<(String, bool)>,
    /// Toggle the caller's like on `docname`.
    on_toggle_like: Callback<String>,
    /// Turn the "only my likes" filter on or off.
    on_toggle_only_liked: Callback<()>,
    /// Signals, not values: the router keeps this component alive when only the
    /// route params change, so a captured `String` would
    /// build row links to the previous doctype under the new URL.
    doctype: Signal<String>,
    module: Signal<String>,
    /// Whether a filter, a search term or "only my likes" is active. An empty
    /// page then means "no matches", not "no records": the table shows a row
    /// with a shortcut to create or to clear the filters.
    #[prop(into)]
    filtered: Signal<bool>,
    /// Permission to create documents, shown as a shortcut in the no-results row.
    #[prop(into)]
    can_create: Signal<bool>,
    /// Navigate to the form view for a brand-new document.
    on_create: Callback<()>,
    /// Clear every active filter, search term and the "only liked" toggle.
    on_reset_filters: Callback<()>,
    /// Ask for the next page when the viewport gets close to the end.
    on_load_more: Callback<()>,
) -> impl IntoView {
    let title_field_name = Memo::new(move |_| {
        meta.get()
            .as_ref()
            .map(|m| title_field(m).to_string())
            .unwrap_or_else(|| "name".into())
    });

    // Fase 5 (`list.row.actions`): the trailing plugin column exists only
    // while the feed declares an extension for the slot on this doctype —
    // no declaration, no extra column, no layout shift for the plain desk.
    // `use_context` (not `expect_context`): standalone renders of this
    // component — the integration tests — simply get no column.
    let ui_ctx = use_context::<UiCtx>();
    let row_actions_declared = Memo::new(move |_| {
        let Some(ctx) = ui_ctx else {
            return false;
        };
        let dt = doctype.get();
        !ctx.extensions_for(
            SLOT_LIST_ROW_ACTIONS,
            (!dt.is_empty()).then_some(dt.as_str()),
        )
        .is_empty()
    });

    // Shared by every row: `use_navigate` returns a closure that has to be
    // cloned into each row's handler, and `Callback` is the handle the rest of
    // the view already uses for owned navigation actions.
    let navigate_fn = use_navigate();
    let navigate: Callback<(String, NavigateOptions)> =
        Callback::new(move |(url, opts): (String, NavigateOptions)| {
            navigate_fn(&url, opts);
        });

    // Only the scroll container's own visible window is rendered. The hook
    // keeps a buffer of rows above and below so fast scrolling does not show
    // blank space.
    let container_ref = NodeRef::<leptos::html::Div>::new();
    let total_rows = Signal::derive(move || rows.with(|rows| rows.len()));
    let virtual_scroll = use_virtual_scroll(container_ref, total_rows);

    // The virtual window is the only scroll signal we need: when its end gets
    // within a few rows of the loaded prefix, ask for the next page. The
    // callback guards concurrent calls itself.
    Effect::new(move |_| {
        let end = virtual_scroll.end_index.get();
        let loaded = rows.with(|rows| rows.len());
        if should_load_more(end, loaded, has_more.get(), loading_more.get()) {
            on_load_more.run(());
        }
    });

    view! {
        <t::TableWrapper class="min-h-0" node_ref=container_ref>
            <t::Table class="min-w-[720px] [&_th:first-child]:pl-4 [&_td:first-child]:pl-4 sm:[&_th:first-child]:pl-6 sm:[&_td:first-child]:pl-6 [&_th:last-child]:pr-4 [&_td:last-child]:pr-4 sm:[&_th:last-child]:pr-6 sm:[&_td:last-child]:pr-6">
                // The header keeps its labels in view while the body scrolls.
                <t::TableHeader class="[&_th]:sticky [&_th]:top-0 [&_th]:z-10 [&_th]:bg-card">
                    <t::TableRow>
                        <t::TableHead class="w-[40px]">
                            <Checkbox
                                aria_label="Seleccionar todos"
                                checked=Signal::derive(move || {
                                    // `rows.with` reads in place; `rows.get` would
                                    // clone every document on the page.
                                    rows.with(|rows| {
                                        let sel = selected.get();
                                        let mut any = false;
                                        let mut all = true;
                                        for row in rows.iter() {
                                            let name = record_name(row);
                                            if name.is_empty() {
                                                continue;
                                            }
                                            any = true;
                                            all &= sel.contains(&name);
                                        }
                                        any && all
                                    })
                                })
                                on_checked_change=on_select_all
                            />
                        </t::TableHead>

                        {move || visible_columns.get().into_iter().map(|c| {
                            let fname = c.fieldname.clone();
                            let label = c.label.clone();
                            let sortable = c.sortable;
                            let afname = fname.clone();

                            if sortable {
                                let f = fname;
                                view! {
                                    <t::TableHead>
                                        <button
                                            type="button"
                                            class="inline-flex items-center gap-1 rounded-sm font-medium outline-none hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring"
                                            on:click=move |_| on_sort.run(f.clone())
                                        >
                                            <span>{label}</span>
                                            <span class="font-normal text-muted-foreground">
                                                {move || {
                                                    let (f, desc) = sort_state.get();
                                                    if f == afname { if desc { "↓" } else { "↑" } } else { "" }
                                                }}
                                            </span>
                                        </button>
                                    </t::TableHead>
                                }.into_any()
                            } else {
                                view! { <t::TableHead>{label}</t::TableHead> }.into_any()
                            }
                        }).collect_view()}

                        <t::TableHead class="w-[100px]">"Estado"</t::TableHead>

                        // Única columna para metadatos (edad + comentarios + likes).
                        // El header solo muestra el conteo de filas y el toggle
                        // del filtro "solo mis favoritos".
                        <t::TableHead class="w-[180px] text-right">
                            <span class="flex items-center justify-end gap-2">
                                <span class="font-normal tabular-nums text-muted-foreground">
                                    {move || {
                                        let shown = rows.with(|rows| rows.len());
                                        format!("{shown} de {}", total.get())
                                    }}
                                </span>
                                <button
                                    type="button"
                                    class=move || {
                                        if only_liked.get() {
                                            "inline-flex size-7 items-center justify-center rounded-md text-destructive outline-none transition-colors hover:bg-muted focus-visible:ring-1 focus-visible:ring-ring"
                                        } else {
                                            "inline-flex size-7 items-center justify-center rounded-md text-muted-foreground outline-none transition-colors hover:bg-muted hover:text-destructive focus-visible:ring-1 focus-visible:ring-ring"
                                        }
                                    }
                                    aria-pressed=move || only_liked.get().to_string()
                                    attr:aria-label="Solo mis favoritos"
                                    title="Solo mis favoritos"
                                    on:click=move |_| on_toggle_only_liked.run(())
                                >
                                    <Icon
                                        name="Heart"
                                        class=move || {
                                            if only_liked.get() {
                                                "size-4 fill-current"
                                            } else {
                                                "size-4"
                                            }
                                        }
                                    />
                                </button>
                            </span>
                        </t::TableHead>

                        // Fase 5: the plugins' per-row action column, at the
                        // end of the row like the actions it holds. Absent
                        // while nothing declares the slot.
                        <Show when=move || row_actions_declared.get()>
                            <t::TableHead class="w-[40px]">
                                <span class="sr-only">"Acciones"</span>
                            </t::TableHead>
                        </Show>
                    </t::TableRow>
                </t::TableHeader>

                <t::TableBody>
                    {move || {
                        let cols_now = visible_columns.get();
                        let actions = row_actions_declared.get();
                        // checkbox + visible columns + Estado + meta (+ the
                        // plugin actions column when it is declared)
                        let colspan = cols_now.len() + 3 + usize::from(actions);

                        // The count is read before the virtual window. Reading
                        // it inside a `rows.with` guard would make
                        // `end_index`'s memo read the same signal again while
                        // the guard is held, and a nested read of an `RwSignal`
                        // is exactly the kind of lock re-entry this table does
                        // not need.
                        let loaded = rows.with(|rows| rows.len());
                        if loaded == 0 {
                            // The empty state is not a table row: a `colspan`
                            // cell inherits the 720px minimum and its message
                            // sits off-screen on a phone. The block is rendered
                            // below the header, outside the table.
                            return AnyView::from(Fragment::new(Vec::new()));
                        }

                        // `start`/`end` are already clamped by the hook; the
                        // second clamp protects against a row list shrinking
                        // between the two signal reads.
                        let start = virtual_scroll.start_index.get().min(loaded);
                        let end = virtual_scroll.end_index.get().min(loaded);
                        let top_height = start * PAGINATION::ROW_HEIGHT;
                        let bottom_height = loaded.saturating_sub(end) * PAGINATION::ROW_HEIGHT;

                        let top_spacer = (top_height > 0).then(|| {
                            view! {
                                <tr aria-hidden="true">
                                    <td
                                        colspan=colspan
                                        class="p-0"
                                        style=format!("height:{top_height}px")
                                    ></td>
                                </tr>
                            }
                            .into_any()
                        });
                        let bottom_spacer = (bottom_height > 0).then(|| {
                            view! {
                                <tr aria-hidden="true">
                                    <td
                                        colspan=colspan
                                        class="p-0"
                                        style=format!("height:{bottom_height}px")
                                    ></td>
                                </tr>
                            }
                            .into_any()
                        });

                        let visible = rows.with(|rows_now| {
                            rows_now
                                .iter()
                                .skip(start)
                                .take(end.saturating_sub(start))
                                .map(|row| {
                                    view! {
                                        <ListRow
                                            row=row.clone()
                                            visible_columns=visible_columns
                                            title_field_name=title_field_name
                                            module=module
                                            doctype=doctype
                                            selected=selected
                                            engagement=engagement
                                            pending_like=pending_like
                                            row_actions_declared=actions
                                            on_select_row=on_select_row
                                            on_toggle_like=on_toggle_like
                                            navigate=navigate
                                        />
                                    }
                                })
                                .collect_view()
                        });

                        view! {
                            <>
                                {top_spacer}
                                {visible}
                                {bottom_spacer}
                                <Show when=move || loading_more.get()>
                                    <tr aria-hidden="true">
                                        <td
                                            colspan=colspan
                                            class="py-3 text-center text-xs text-muted-foreground"
                                        >
                                            <span class="inline-flex items-center justify-center gap-2">
                                                <Spinner class="size-4 text-muted-foreground" />
                                                "Cargando más…"
                                            </span>
                                        </td>
                                    </tr>
                                </Show>
                            </>
                        }
                        .into_any()
                    }}
                </t::TableBody>
            </t::Table>
            {move || rows.with(Vec::is_empty).then(|| view! {
                <ListEmptyBlock
                    filtered=filtered
                    can_create=can_create
                    on_create=on_create
                    on_reset_filters=on_reset_filters
                />
            })}
        </t::TableWrapper>
    }
}
