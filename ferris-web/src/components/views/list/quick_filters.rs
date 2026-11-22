//! Quick filters: one compact row per seeded standard-filter field, each with
//! its own operator and value.
//!
//! These rows and the advanced dropdown write different query slices on
//! purpose: `ViewQuery::quick_filters` and `ViewQuery::filters`. Applying the
//! dropdown therefore replaces only its own rows, and a quick filter stays in
//! place. `ViewQuery` merges both when it builds the list request.
//!
//! The field seed is the doctype's standard-filter set, not `list_view`:
//! `list_view` only decides which columns the table opens with. The dropdown's
//! field selector is the path to every other field.

use leptos::prelude::*;

use super::filter_ops::{FilterValueEditor, OperatorPicker, operators_for};
use crate::api::meta::Column;
use crate::api::types::{Filter, FilterOp};
use crate::components::ui::button_group::{ButtonGroup, ButtonGroupText};

/// Build the wire filter for one row. `None` means "this row is empty, drop
/// it", which is what keeps a cleared box from sending `field equals ""`.
fn row_filter(field: &str, op: FilterOp, raw: String) -> Option<Filter> {
    if op == FilterOp::In {
        let values: Vec<String> = raw
            .split(',')
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect();
        if values.is_empty() {
            return None;
        }
        return Some(Filter {
            field: field.to_string(),
            op,
            value: serde_json::to_string(&values).unwrap_or_default(),
            or: false,
        });
    }
    if raw.trim().is_empty() {
        return None;
    }
    Some(Filter {
        field: field.to_string(),
        op,
        value: raw,
        or: false,
    })
}

#[component]
pub fn ListQuickFilters(
    #[prop(into)] fields: Signal<Vec<Column>>,
    #[prop(into)] filters: Signal<Vec<Filter>>,
    on_change: Callback<Vec<Filter>>,
) -> impl IntoView {
    view! {
        // On a phone the seeded rows are one scrollable strip: each row is a
        // connected group that must not be squeezed, and the toolbar above it
        // must not push the page sideways. From `sm` up the rows wrap as before.
        <div class="flex w-full min-w-0 items-center gap-2 overflow-x-auto sm:w-auto sm:flex-wrap sm:overflow-visible">
            <For
                each=move || fields.get()
                key=|field| field.fieldname.clone()
                children=move |field| {
                    view! {
                        <QuickFilterRow field=field filters=filters on_change=on_change />
                    }
                }
            />
        </div>
    }
}

#[component]
fn QuickFilterRow(
    field: Column,
    #[prop(into)] filters: Signal<Vec<Filter>>,
    on_change: Callback<Vec<Filter>>,
) -> impl IntoView {
    let fieldname = field.fieldname.clone();
    let label = field.label.clone();
    let fieldtype = field.fieldtype.clone();
    let options = field.options.clone();
    let operators = operators_for(&fieldtype);
    let default_op = operators.first().copied().unwrap_or(FilterOp::Like);
    let input_id = format!("list-quick-filter-{fieldname}");

    let operator = RwSignal::new(default_op);
    let value = RwSignal::new(String::new());
    let focused = RwSignal::new(false);

    // Mirror the shared list: a view restore seeds it before this row mounts,
    // and the advanced dropdown can edit or remove a filter on this field.
    let synced_field = fieldname.clone();
    Effect::new(move |_| {
        let current = filters
            .get()
            .into_iter()
            .find(|filter| filter.field == synced_field);
        let next_op = current.as_ref().map_or(default_op, |filter| filter.op);
        let next_value = current.map_or_else(String::new, |filter| filter.value);
        if operator.get_untracked() != next_op {
            operator.set(next_op);
        }
        // Do not pull the rug from under a focused input: the local value is
        // the user's edit until `change` commits it.
        if !focused.get() && value.get_untracked() != next_value {
            value.set(next_value);
        }
    });

    let commit = {
        let fieldname = fieldname.clone();
        Callback::new(move |(op, raw): (FilterOp, String)| {
            let mut next: Vec<Filter> = filters
                .get_untracked()
                .into_iter()
                .filter(|filter| filter.field != fieldname)
                .collect();
            if let Some(filter) = row_filter(&fieldname, op, raw) {
                next.push(filter);
            }
            on_change.run(next);
        })
    };

    let on_pick = {
        Callback::new(move |op: FilterOp| {
            operator.set(op);
            commit.run((op, value.get_untracked()));
        })
    };

    let on_commit = {
        Callback::new(move |()| {
            commit.run((operator.get_untracked(), value.get_untracked()));
        })
    };

    let placeholder = format!("Valor de {label}");
    let aria_label = placeholder.clone();
    let picker_fieldtype = fieldtype.clone();

    view! {
        // The external row is one connected outline control: label, operator and
        // value all keep their border but sit on a transparent background, so
        // the toolbar does not look like a stack of filled boxes. `bg-transparent`
        // overrides the fills these components bring from their own layer
        // (`bg-muted` on `ButtonGroupText`, `--background` on the outline button,
        // `bg-background` on `SelectNative`). The rows inside `filter_group` keep
        // their filled look on purpose.
        <ButtonGroup class="inline-flex shrink-0 items-stretch">
            <ButtonGroupText class="h-8 max-w-[140px] truncate rounded-r-none border-r-0 bg-transparent px-2.5 text-xs max-md:h-10">
                {label}
            </ButtonGroupText>
            <span class="contents">
                <OperatorPicker
                    fieldtype=picker_fieldtype
                    // The picker receives the operator once and reports
                    // changes through `on_pick`; a tracked read here would only
                    // warn about reading outside a reactive context.
                    operator=operator.get_untracked()
                    trigger_class="h-8 gap-1 rounded-none border-x-0 bg-transparent px-2"
                    on_pick=on_pick
                />
            </span>
            <div
                class="contents"
                on:focusin=move |_| focused.set(true)
                on:focusout=move |_| focused.set(false)
            >
                <FilterValueEditor
                    id=input_id
                    fieldtype=fieldtype
                    options=options.unwrap_or_default()
                    value=value
                    // `max-md:text-base`: the input keeps 16px on a phone so
                    // focusing it does not make iOS Safari zoom the page.
                    class="h-8 w-40 rounded-l-none border-l-0 bg-transparent text-sm max-md:text-base"
                    placeholder=placeholder
                    aria_label=aria_label
                    on_commit=on_commit
                />
            </div>
        </ButtonGroup>
    }
}
