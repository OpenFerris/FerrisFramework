//! Child-table editor for the form view.
//!
//! Frappe's `Table` field is a grid of rows whose shape is another DocType. The
//! form used to show a dashed "pending" notice and `build_form_payload` dropped
//! the field entirely — editing a parent with child rows and saving erased
//! them. This component closes both halves:
//!
//! * the rows are loaded from the table field's JSON array;
//! * each cell is edited with the widget that fits its child fieldtype
//!   (checkbox, select, number or text);
//! * every change rewrites the field's `value` signal, so the normal
//!   [`super::super::super::views::services::build_form_payload`] sends the
//!   array and an empty table sends `[]` (clearing rows reaches the engine);
//! * `idx` is renumbered on every edit, matching Frappe's child-row contract.
//!
//! The child DocType is read from the field's `options`, exactly where Frappe
//! stores it. No doctype name is hardcoded.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::Value;

use crate::api::meta::label_of;
use crate::api::{DocStore, DynField, DynMeta};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::icon::Icon;
use crate::components::ui::skeleton::Skeleton;
use crate::i18n::{t, use_i18n};
use crate::views::services::FormField;

/// Fields the child grid never edits: layout markers, nested tables and the
/// engine's parent bookkeeping.
fn is_layout(fieldtype: &str) -> bool {
    matches!(
        fieldtype,
        "Tab Break" | "Section Break" | "Column Break" | "Heading" | "HTML" | "Button"
    )
}

fn is_bookkeeping(fieldname: &str) -> bool {
    matches!(
        fieldname,
        "name"
            | "owner"
            | "creation"
            | "modified"
            | "modified_by"
            | "docstatus"
            | "parent"
            | "parentfield"
            | "parenttype"
            | "idx"
    )
}

/// Parse the field's stored value into rows.
///
/// An empty value is an empty table; anything that is not a JSON array is an
/// error rather than an empty table. Silently reading it as "no rows" and then
/// mirroring `[]` back into the field (the effect below) would erase the
/// document's child rows on the next save.
fn parse_rows(
    raw: &str,
    i18n: &(impl crate::i18n::Translate + Sync),
) -> Result<Vec<Value>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str::<Vec<Value>>(raw)
        .map_err(|_| i18n.t("components.form.child_table.parse_error"))
}

/// Stamp `idx` from the current order, matching Frappe's contract for child
/// rows. Adding or removing a row leaves a gap otherwise, and the engine's
/// renumbering on read would disagree with what the form just sent.
fn renumber(rows: &mut [Value]) {
    for (index, row) in rows.iter_mut().enumerate() {
        if let Value::Object(map) = row {
            map.insert(
                "idx".to_string(),
                Value::from(i64::try_from(index + 1).unwrap_or(i64::MAX)),
            );
        }
    }
}

fn cell_text(row: &Value, fieldname: &str, fieldtype: &str) -> String {
    match row.get(fieldname) {
        Some(Value::String(text)) => text.clone(),
        // An `Int` cell never shows a fractional part: `4.0` in a quantity
        // column reads as a float that the field is not. The engine may hand
        // back a JSON float (`4.0`), whose `as_i64` is `None`.
        Some(Value::Number(number)) if fieldtype == "Int" => number
            .as_i64()
            .or_else(|| number.as_f64().map(|value| value.trunc() as i64))
            .map_or_else(|| number.to_string(), |value| value.to_string()),
        Some(Value::Null) | None => String::new(),
        Some(value) => value.to_string(),
    }
}

fn cell_bool(row: &Value, fieldname: &str) -> bool {
    match row.get(fieldname) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_i64().is_some_and(|n| n != 0),
        Some(Value::String(text)) => text == "1" || text.eq_ignore_ascii_case("true"),
        Some(Value::Null) | Some(Value::Array(_)) | Some(Value::Object(_)) | None => false,
    }
}

fn default_cell(field: &DynField) -> Value {
    let default = field.default.clone().unwrap_or_default();
    match field.fieldtype.as_str() {
        "Check" => Value::Bool(default == "1" || default.eq_ignore_ascii_case("true")),
        "Int" => default
            .trim()
            .replace(['.', ','], "")
            .parse::<i64>()
            .map(Value::from)
            .ok()
            .unwrap_or(Value::Null),
        "Float" | "Currency" | "Percent" => default
            .parse::<f64>()
            .map(Value::from)
            .unwrap_or(Value::Null),
        _ if default.is_empty() => Value::Null,
        _ => Value::String(default),
    }
}

/// The editable grid of one `Table` field.
#[component]
pub fn ChildTableField(field: FormField, read_only: bool) -> impl IntoView {
    let i18n = use_i18n();
    let child_doctype = field.options.clone().unwrap_or_default();
    let label = field.label.clone();
    let value = field.value;
    let (parsed_rows, parse_error) = match parse_rows(&value.get_untracked(), &i18n) {
        Ok(rows) => (rows, None),
        Err(message) => (Vec::new(), Some(message)),
    };
    let parsed_ok = parse_error.is_none();
    let rows = RwSignal::new(parsed_rows);
    let meta = RwSignal::new(None::<DynMeta>);
    let loading = RwSignal::new(true);
    let error = RwSignal::new(parse_error);

    if !parsed_ok {
        // The stored value is not a table: say so and leave it untouched.
        loading.set(false);
    } else if !child_doctype.trim().is_empty() {
        let child_for_load = child_doctype.clone();
        let load_i18n = i18n;
        Effect::new(move |_| {
            let doctype = child_for_load.clone();
            let i18n = load_i18n;
            spawn_local(async move {
                let result = match DocStore::runtime() {
                    Ok(store) => store.meta(&doctype).await,
                    Err(message) => Err(message),
                };
                loading.set(false);
                match result {
                    Ok(Some(child)) => meta.set(Some(child)),
                    Ok(None) => error.set(Some(i18n.t_with(
                        "components.form.child_table.unregistered_doctype",
                        &[("doctype", &doctype)],
                    ))),
                    Err(message) => error.set(Some(message)),
                }
            });
        });
    } else {
        loading.set(false);
        error.set(Some(i18n.t("components.form.child_table.missing_doctype")));
    }

    // Mirror every edit into the field's value signal. The initial effect
    // serializes the rows it just parsed; writing an identical string would
    // still mark the form dirty, so only a real change is pushed. A value that
    // failed to parse is never mirrored: it is the document's data, not ours.
    Effect::new(move |_| {
        if !parsed_ok {
            return;
        }
        let current = rows.get();
        if let Ok(serialized) = serde_json::to_string(&current)
            && value.get_untracked() != serialized
        {
            value.set(serialized);
        }
    });

    let columns = move || {
        meta.get()
            .map(|meta| {
                let editable: Vec<DynField> = meta
                    .fields
                    .iter()
                    .filter(|field| {
                        !field.hidden
                            && !is_layout(&field.fieldtype)
                            && !is_bookkeeping(&field.fieldname)
                            && field.fieldtype != "Table"
                            // The standard `tags` field is on every DocType;
                            // a tags cell on every child row is noise, not data.
                            && field.fieldname != "tags"
                    })
                    .cloned()
                    .collect();
                let listed: Vec<DynField> = editable
                    .iter()
                    .filter(|field| field.in_list_view)
                    .cloned()
                    .collect();
                if listed.is_empty() {
                    editable.into_iter().take(6).collect()
                } else {
                    listed
                }
            })
            .unwrap_or_default()
    };

    let update = Callback::new(move |(index, fieldname, cell): (usize, String, Value)| {
        rows.update(|rows| {
            if let Some(Value::Object(map)) = rows.get_mut(index) {
                map.insert(fieldname, cell);
            }
        });
    });

    let add_row = move |_| {
        let Some(meta) = meta.get() else {
            return;
        };
        let mut row = serde_json::Map::new();
        for field in &meta.fields {
            if is_layout(&field.fieldtype)
                || is_bookkeeping(&field.fieldname)
                || field.fieldname == "tags"
            {
                continue;
            }
            row.insert(field.fieldname.clone(), default_cell(field));
        }
        rows.update(|rows| {
            rows.push(Value::Object(row));
            // `idx` follows the position, not the insertion order.
            renumber(rows);
        });
    };

    let remove_row = Callback::new(move |index: usize| {
        rows.update(|rows| {
            if index < rows.len() {
                rows.remove(index);
                renumber(rows);
            }
        });
    });

    view! {
        <div class="flex flex-col gap-2">
            <div class="flex flex-wrap items-center justify-between gap-2">
                <h4 class="text-sm font-medium text-foreground">{label}</h4>
                {(!read_only).then(|| view! {
                    <Button
                        variant=ButtonVariant::Outline
                        size=ButtonSize::Sm
                        on:click=add_row
                    >
                        <Icon name="Plus" class="size-4" />
                        {t("components.form.child_table.add_row")}
                    </Button>
                })}
            </div>

            <Show when=move || loading.get()>
                <Skeleton class="h-20 w-full rounded-lg" />
            </Show>
            <Show when=move || error.get().is_some()>
                <p class="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                    {move || error.get().unwrap_or_default()}
                </p>
            </Show>

            <Show when=move || !loading.get() && error.get().is_none()>
                <div class="overflow-x-auto rounded-lg border border-border">
                    <table class="w-full text-sm">
                        <thead class="bg-muted/40">
                            <tr>
                                {move || columns().into_iter().map(|field| {
                                    let label = label_of(&field);
                                    view! {
                                        <th scope="col" class="px-3 py-2 text-left font-medium whitespace-nowrap">
                                            {label}
                                        </th>
                                    }
                                }).collect_view()}
                                {(!read_only).then(|| view! {
                                    <th scope="col" class="w-10 px-2 py-2"><span class="sr-only">{t("components.form.child_table.actions")}</span></th>
                                })}
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let columns = columns();
                                let rows_value = rows.get();
                                if rows_value.is_empty() {
                                    return view! {
                                        <tr>
                                            <td
                                                class="px-3 py-6 text-center text-muted-foreground"
                                                colspan=(columns.len() + if read_only { 0 } else { 1 }).to_string()
                                            >
                                                {t("components.form.child_table.empty")}
                                            </td>
                                        </tr>
                                    }.into_any();
                                }
                                rows_value
                                    .into_iter()
                                    .enumerate()
                                    .map(|(index, row)| {
                                        let cells = columns.iter().map(|field| {
                                            let fieldname = field.fieldname.clone();
                                            let fieldtype = field.fieldtype.clone();
                                            let options = field.options.clone().unwrap_or_default();
                                            let cell_label = label_of(field);
                                            // Field names are unique per parent
                                            // document, so this id is unique on
                                            // the page without naming the child
                                            // DocType (which the render closure
                                            // only borrows).
                                            let cell_id = format!("child-{fieldname}-{index}");
                                            if read_only {
                                                let text = if fieldtype == "Check" {
                                                    if cell_bool(&row, &fieldname) {
                                                        t("components.list.filter.yes")
                                                    } else {
                                                        t("components.list.filter.no")
                                                    }
                                                } else {
                                                    cell_text(&row, &fieldname, &fieldtype)
                                                };
                                                return view! {
                                                    <td class="px-3 py-2 text-muted-foreground">{text}</td>
                                                }.into_any();
                                            }
                                            let rows_signal = rows;
                                            match fieldtype.as_str() {
                                                "Check" => {
                                                    let checked_name = fieldname.clone();
                                                    let change_name = fieldname.clone();
                                                    view! {
                                                        <td class="px-3 py-2">
                                                            <Checkbox
                                                                aria_label=cell_label
                                                                checked=Signal::derive(move || {
                                                                    rows_signal
                                                                        .get()
                                                                        .get(index)
                                                                        .is_some_and(|row| cell_bool(row, &checked_name))
                                                                })
                                                                on_checked_change=Callback::new(move |checked: bool| {
                                                                    update.run((index, change_name.clone(), Value::Bool(checked)));
                                                                })
                                                            />
                                                        </td>
                                                    }.into_any()
                                                }
                                                "Select" => {
                                                    let value_name = fieldname.clone();
                                                    let change_name = fieldname.clone();
                                                    let options = options
                                                        .split('\n')
                                                        .map(str::trim)
                                                        .filter(|option| !option.is_empty())
                                                        .map(str::to_string)
                                                        .collect::<Vec<_>>();
                                                    view! {
                                                        <td class="px-2 py-1.5">
                                                            <select
                                                                id=cell_id
                                                                class="ferris-input h-8 w-full min-w-28 rounded-md border px-2 py-1 text-sm"
                                                                aria-label=cell_label
                                                                prop:value=move || {
                                                                    rows_signal
                                                                        .get()
                                                                        .get(index)
                                                                        .map(|row| {
                                                                            cell_text(
                                                                                row,
                                                                                &value_name,
                                                                                &fieldtype,
                                                                            )
                                                                        })
                                                                        .unwrap_or_default()
                                                                }
                                                                on:change=move |ev: leptos::ev::Event| {
                                                                    update.run((
                                                                        index,
                                                                        change_name.clone(),
                                                                        Value::String(event_target_value(&ev)),
                                                                    ));
                                                                }
                                                            >
                                                                <option value="">"—"</option>
                                                                {options.into_iter().map(|option| {
                                                                    let label = option.clone();
                                                                    view! { <option value=option>{label}</option> }
                                                                }).collect_view()}
                                                            </select>
                                                        </td>
                                                    }.into_any()
                                                }
                                                _ => {
                                                    let value_name = fieldname.clone();
                                                    let change_name = fieldname.clone();
                                                    // `StoredValue` is `Copy`: the two closures below both
                                                    // need the fieldtype and a moved `String` would be
                                                    // `FnOnce`.
                                                    let cell_type = StoredValue::new(fieldtype.clone());
                                                    let numeric = matches!(
                                                        fieldtype.as_str(),
                                                        "Int" | "Float" | "Currency" | "Percent"
                                                    );
                                                    view! {
                                                        <td class="px-2 py-1.5">
                                                            <input
                                                                id=cell_id
                                                                class="ferris-input h-8 w-full min-w-32 rounded-md border px-2 py-1 text-sm"
                                                                aria-label=cell_label
                                                                prop:value=move || {
                                                                    rows_signal
                                                                        .get()
                                                                        .get(index)
                                                                        .map(|row| {
                                                                            cell_type.with_value(|t| {
                                                                                cell_text(row, &value_name, t)
                                                                            })
                                                                        })
                                                                        .unwrap_or_default()
                                                                }
                                                                on:change=move |ev| {
                                                                    let text = event_target_value(&ev);
                                                                    let cell = if numeric {
                                                                        // Same separator policy as the form's
                                                                        // numeric fields: a comma is the locale's
                                                                        // decimal mark, and an `Int` cell ignores
                                                                        // the separator instead of truncating
                                                                        // (`2,5` → `25`, never `2`).
                                                                        let fieldtype = cell_type.get_value();
                                                                        let text = text.trim();
                                                                        if fieldtype == "Int" {
                                                                            text.replace(['.', ','], "")
                                                                                .parse::<i64>()
                                                                                .map(Value::from)
                                                                                .unwrap_or(Value::Null)
                                                                        } else {
                                                                            text.replace(',', ".")
                                                                                .parse::<f64>()
                                                                                .map(Value::from)
                                                                                .unwrap_or(Value::Null)
                                                                        }
                                                                    } else {
                                                                        Value::String(text)
                                                                    };
                                                                    update.run((index, change_name.clone(), cell));
                                                                }
                                                            />
                                                        </td>
                                                    }.into_any()
                                                }
                                            }
                                        }).collect_view();
                                        let remove = remove_row;
                                        view! {
                                            <tr class="border-t border-border">
                                                {cells}
                                                {(!read_only).then(|| view! {
                                                    <td class="px-2 py-1.5 text-right">
                                                        <Button
                                                            variant=ButtonVariant::Ghost
                                                            size=ButtonSize::IconSm
                                                            attr:aria-label=t("components.form.child_table.remove_row")
                                                            on:click=move |_| remove.run(index)
                                                        >
                                                            <Icon name="Trash2" class="size-4" />
                                                        </Button>
                                                    </td>
                                                })}
                                            </tr>
                                        }.into_any()
                                    })
                                    .collect_view()
                                    .into_any()
                            }}
                        </tbody>
                    </table>
                </div>
            </Show>
        </div>
    }
}
