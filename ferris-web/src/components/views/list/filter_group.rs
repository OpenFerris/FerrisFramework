//! Filter dropdown: a two-segment group — the left segment opens the filter
//! menu (one row per filter), the right segment drops them all.

use leptos::prelude::*;

use super::filter_ops::{FilterValueEditor, OperatorPicker, operators_for};
use crate::api::meta::Column;
use crate::api::types::{Filter, FilterOp};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::button_group::ButtonGroup;
use crate::components::ui::dropdown_menu::{
    DropdownMenu, DropdownMenuAlign, DropdownMenuContent, DropdownMenuTrigger,
};
use crate::components::ui::select_native::SelectNative;
use crate::components::ui::separator::Separator;
use crate::i18n::{t, use_i18n};

use icons::{Filter as FilterIcon, Plus, X};

/// One staged filter row. The value lives in its own signal so typing does not
/// rebuild the row.
#[derive(Clone)]
struct FilterRow {
    id: u64,
    field: String,
    label: String,
    fieldtype: String,
    options: Option<String>,
    operator: FilterOp,
    value: RwSignal<String>,
}

impl FilterRow {
    fn new(field: Column, id: u64) -> Self {
        // A new row starts on the operator the field actually offers. A
        // `Datetime` row seeded with the generic `like` showed a token the
        // operator menu does not contain.
        let operator = operators_for(&field.fieldtype)
            .first()
            .copied()
            .unwrap_or(FilterOp::Like);
        Self {
            id,
            field: field.fieldname,
            label: field.label,
            fieldtype: field.fieldtype,
            options: field.options,
            operator,
            value: RwSignal::new(String::new()),
        }
    }

    fn from_filter(f: &Filter, fields: &[Column], id: u64) -> Option<Self> {
        let col = fields.iter().find(|c| c.fieldname == f.field)?;
        let mut row = Self::new(col.clone(), id);
        row.operator = f.op;
        row.value.set(f.value.clone());
        Some(row)
    }

    fn operators(&self) -> Vec<FilterOp> {
        operators_for(&self.fieldtype)
    }

    fn to_filter(&self) -> Option<Filter> {
        let op = self.operator;
        let value = self.value.get();
        let trimmed = value.trim();

        match op {
            FilterOp::Equals => (!trimmed.is_empty()).then(|| Filter {
                field: self.field.clone(),
                op,
                value,
                or: false,
            }),
            FilterOp::In => {
                let vals: Vec<String> = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                (!vals.is_empty()).then(|| Filter {
                    field: self.field.clone(),
                    op,
                    value: serde_json::to_string(&vals).unwrap_or_default(),
                    or: false,
                })
            }
            _ => (!trimmed.is_empty()).then(|| Filter {
                field: self.field.clone(),
                op,
                value,
                or: false,
            }),
        }
    }
}

#[component]
pub fn ListFilterGroup(
    #[prop(into)] filters: Signal<Vec<Filter>>,
    #[prop(into)] meta_fields: Signal<Vec<Column>>,
    on_apply: Callback<Vec<Filter>>,
    /// Drops every restriction, not just the rows in this panel: the quick
    /// rows and the "only liked" toggle live outside it, so the button has to
    /// ask the list service to clear them.
    on_clear: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    let rows = RwSignal::new(Vec::<FilterRow>::new());
    let next_id = RwSignal::new(1u64);
    let alloc_id = move || {
        let id = next_id.get_untracked();
        next_id.set(id + 1);
        id
    };

    // Each `FilterRow` carries its own `RwSignal` for the staged value, and an
    // arena item is registered with whatever owner is current when it is
    // created. That matters here because the initialization `Effect` below
    // re-runs whenever the field list changes — navigating to another doctype
    // re-fetches metadata, which resets `filter_fields` — and `Effect` runs each
    // pass through `Owner::with_cleanup`, which disposes (step 3) everything the
    // previous pass registered *before* running again. A row created inside the
    // effect therefore lost its value signal on the next run while `rows` still
    // held it, and the next `read_only()` panicked with "you tried to access a
    // reactive value ... which has already been disposed"
    //. Allocating the rows under the component's
    // own owner keeps them alive until the filter group itself unmounts.
    let owner = match Owner::current() {
        Some(owner) => owner,
        None => Owner::new(),
    };

    // Stage the persisted filters once the field list is known. Always at
    // least one row.
    let init_owner = owner.clone();
    let initialized = StoredValue::new(false);
    Effect::new(move |_| {
        if initialized.get_value() {
            return;
        }
        let fields = meta_fields.get();
        if fields.is_empty() {
            return;
        }
        initialized.set_value(true);

        let mut staged: Vec<FilterRow> = filters
            .get_untracked()
            .iter()
            .filter_map(|f| init_owner.with(|| FilterRow::from_filter(f, &fields, alloc_id())))
            .collect();
        if staged.is_empty() {
            staged.push(init_owner.with(|| FilterRow::new(fields[0].clone(), alloc_id())));
        }
        rows.set(staged);
    });

    let add_owner = owner.clone();
    let on_add = Callback::new(move |_| {
        rows.update(|staged| {
            let fields = meta_fields.get();
            if fields.is_empty() {
                return;
            }
            let used: Vec<&str> = staged.iter().map(|r| r.field.as_str()).collect();
            let pick = fields
                .iter()
                .find(|f| !used.contains(&f.fieldname.as_str()))
                .or_else(|| fields.first())
                .cloned();
            if let Some(field) = pick {
                staged.push(add_owner.with(|| FilterRow::new(field, alloc_id())));
            }
        });
    });

    let on_remove = Callback::new(move |id: u64| {
        rows.update(|staged| {
            if staged.len() > 1 {
                staged.retain(|r| r.id != id);
            }
        });
    });

    let on_apply_click = Callback::new(move |_| {
        let staged = rows.get();
        on_apply.run(staged.iter().filter_map(|r| r.to_filter()).collect());
    });

    let clear_owner = owner.clone();
    let clear_all = Callback::new(move |_| {
        // Empty the staged rows first so the panel does not keep a value the
        // list no longer filters on, then let the service drop the quick rows,
        // the advanced filters, the search term and the like toggle in a
        // single query update.
        rows.update(|staged| {
            staged.clear();
            if let Some(first) = meta_fields.get_untracked().first() {
                staged.push(clear_owner.with(|| FilterRow::new(first.clone(), alloc_id())));
            }
        });
        on_clear.run(());
    });

    view! {
        <ButtonGroup class="inline-flex items-stretch">
            <DropdownMenu align=DropdownMenuAlign::End>
                <DropdownMenuTrigger as_child=true>
                    <Button variant=ButtonVariant::Outline size=ButtonSize::Sm>
                        <FilterIcon class="size-4" />
                        {t("components.list.filter.title")}
                    </Button>
                </DropdownMenuTrigger>

                <DropdownMenuContent
                    class="w-[600px] max-w-[calc(100vw-1rem)] p-0"
                    animated=false
                >
                    // The panel is anchored to a button near the edge of the
                    // viewport on a phone, so its height is capped against the
                    // dynamic viewport too and the rows scroll inside it.
                    <div class="max-h-[60dvh] overflow-y-auto sm:max-h-[320px]">
                        <For
                            each=move || rows.get()
                            key=|r| r.id
                            children=move |row: FilterRow| {
                                let row_id = row.id;
                                let row_i18n = i18n;
                                view! {
                                    {move || {
                                        let staged = rows.get();
                                        let Some(current) =
                                            staged.iter().find(|r| r.id == row_id).cloned()
                                        else {
                                            return view! { <div></div> }.into_any();
                                        };

                                        let field_value =
                                            RwSignal::new(current.field.clone()).read_only();
                                        let value = current.value;
                                        let fieldtype = current.fieldtype.clone();
                                        let options = current.options.clone();
                                        let label = current.label.clone();
                                        let field_options = meta_fields.get();
                                        let current_id = current.id;
                                        let selected_op = current.operator;
                                        let removable = staged.len() > 1;

                                        let editor = view! {
                                            <FilterValueEditor
                                                id=format!("filter-value-{}", current_id)
                                                fieldtype=fieldtype.clone()
                                                options=options.unwrap_or_default()
                                                value=value
                                                class="h-8 w-full"
                                                placeholder=row_i18n.t_with(
                                                    "components.list.filter.value_of",
                                                    &[("field", &label)],
                                                )
                                                aria_label=row_i18n.t_with(
                                                    "components.list.filter.value_of",
                                                    &[("field", &label)],
                                                )
                                            />
                                        }
                                            .into_any();

                                        view! {
                                            <div class="flex flex-wrap items-center gap-2 border-b border-border/60 px-3 py-2">
                                                <label
                                                    class="sr-only"
                                                    for=format!("filter-field-{}", current_id)
                                                >
                                                    {t("components.list.filter.field")}
                                                </label>
                                                <ButtonGroup class="inline-flex min-w-0 max-w-full items-stretch">
                                                    <SelectNative
                                                        id=format!("filter-field-{}", current_id)
                                                        value=field_value
                                                        on_change=Callback::new(
                                                            move |ev: leptos::ev::Event| {
                                                                let picked = event_target_value(&ev);
                                                                let fields_now =
                                                                    meta_fields.get_untracked();
                                                                rows.update(|staged| {
                                                                    let Some(row) =
                                                                        staged.iter_mut().find(
                                                                            |r| r.id == current_id,
                                                                        )
                                                                    else {
                                                                        return;
                                                                    };
                                                                    let Some(field) = fields_now
                                                                        .iter()
                                                                        .find(
                                                                            |f| f.fieldname == picked,
                                                                        )
                                                                    else {
                                                                        return;
                                                                    };
                                                                    row.field = field.fieldname.clone();
                                                                    row.label = field.label.clone();
                                                                    row.fieldtype =
                                                                        field.fieldtype.clone();
                                                                    row.options = field.options.clone();
                                                                    let next_op = row
                                                                        .operators()
                                                                        .first()
                                                                        .copied()
                                                                        .unwrap_or(FilterOp::Like);
                                                                    row.operator = next_op;
                                                                    row.value.set(String::new());
                                                                });
                                                            },
                                                        )
                                                        // Fixed width from `sm` up: the selector
                                                        // must not resize with the option labels.
                                                        // On a phone it takes a share of the row so
                                                        // the operator and the value still fit.
                                                        class="h-8 min-w-0 w-28 sm:w-36 rounded-r-none border-r-0"
                                                    >
                                                        {field_options
                                                            .iter()
                                                            .map(|f| {
                                                                view! {
                                                                    <option value={f.fieldname.clone()}>
                                                                        {f.label.clone()}
                                                                    </option>
                                                                }
                                                            })
                                                            .collect_view()}
                                                    </SelectNative>

                                                    // The operator menu lives inside the
                                                    // filter panel's own dropdown. Stop
                                                    // the click here so choosing an
                                                    // operator does not also dismiss the
                                                    // whole panel.
                                                    <span
                                                        class="contents"
                                                        on:click=move |ev: leptos::ev::MouseEvent| {
                                                            ev.stop_propagation();
                                                        }
                                                    >
                                                        <OperatorPicker
                                                            fieldtype=fieldtype
                                                            operator=selected_op
                                                            trigger_class="h-8 gap-1 rounded-l-none px-2"
                                                            on_pick=Callback::new(
                                                                move |op: FilterOp| {
                                                                    rows.update(|staged| {
                                                                        if let Some(row) = staged
                                                                            .iter_mut()
                                                                            .find(|r| r.id == current_id)
                                                                        {
                                                                            row.operator = op;
                                                                        }
                                                                    });
                                                                },
                                                            )
                                                        />
                                                    </span>
                                                </ButtonGroup>

                                                <div class="min-w-0 flex-1 basis-[10rem]">{editor}</div>

                                                {if removable {
                                                    view! {
                                                        <Button
                                                            variant=ButtonVariant::Ghost
                                                            size=ButtonSize::Sm
                                                            class="shrink-0 px-1.5"
                                                            attr:aria-label=t("components.list.filter.remove")
                                                            on:click=move |_| {
                                                                on_remove.run(current_id);
                                                            }
                                                        >
                                                            <X class="size-4" />
                                                        </Button>
                                                    }
                                                        .into_any()
                                                } else {
                                                    view! { <span class="w-6 shrink-0"></span> }
                                                        .into_any()
                                                }}
                                            </div>
                                        }
                                        .into_any()
                                    }}
                                }
                            }
                        />
                    </div>

                    <Separator />

                    <div class="flex flex-wrap items-center justify-between gap-2 px-3 py-2">
                        <Button
                            variant=ButtonVariant::Ghost
                            size=ButtonSize::Sm
                            on:click=move |_| on_add.run(())
                        >
                            <Plus class="size-4" />
                            {t("components.list.filter.add")}
                        </Button>
                        <div class="flex items-center gap-2">
                            <Button
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::Sm
                                on:click=move |_| clear_all.run(())
                            >
                                {t("components.list.filter.clear")}
                            </Button>
                            <Button
                                variant=ButtonVariant::Accent
                                size=ButtonSize::Sm
                                attr:data-dropdown-close="true"
                                on:click=move |_| on_apply_click.run(())
                            >
                                {t("components.list.filter.apply")}
                            </Button>
                        </div>
                    </div>
                </DropdownMenuContent>
            </DropdownMenu>

            <Button
                variant=ButtonVariant::Outline
                size=ButtonSize::IconSm
                attr:aria-label=t("components.list.filter.clear_all")
                on:click=move |_| clear_all.run(())
            >
                <X class="size-4" />
            </Button>
        </ButtonGroup>
    }
}
