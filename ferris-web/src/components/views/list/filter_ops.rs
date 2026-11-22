//! Shared pieces of the list filter UI: the operator vocabulary, the operator
//! menu and the value editor.
//!
//! The advanced filter dropdown and the quick-filter row outside it must offer
//! the same choices; keeping one copy here is what stops the two from drifting
//! (a new operator reaching only one of them would silently change what the
//! other can express).

use leptos::prelude::*;
use tw_merge::tw_merge;

use crate::api::types::FilterOp;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dropdown_menu::{
    DropdownMenu, DropdownMenuAction, DropdownMenuAlign, DropdownMenuContent, DropdownMenuGroup,
    DropdownMenuItem, DropdownMenuTrigger,
};
use crate::components::ui::input::{Input, InputType};
use crate::components::ui::select_native::SelectNative;

use icons::{
    CaseSensitive, Check, ChevronDown, ChevronUp, ChevronsDown, ChevronsUp, Equal, List as ListIcon,
};

/// The operators that make sense for a fieldtype.
///
/// Select-like fields compare by equality and membership; ordered fields get
/// the range operators; anything else is free text. The first entry is the
/// default for a newly added filter row.
pub(super) fn operators_for(fieldtype: &str) -> Vec<FilterOp> {
    match fieldtype {
        "Select" | "Link" => vec![FilterOp::Equals, FilterOp::In],
        "Date" | "Datetime" | "Int" | "Float" | "Currency" | "Percent" => vec![
            FilterOp::Equals,
            FilterOp::Gt,
            FilterOp::Gte,
            FilterOp::Lt,
            FilterOp::Lte,
        ],
        "Check" => vec![FilterOp::Equals],
        _ => vec![FilterOp::Like, FilterOp::Equals],
    }
}

/// The human label for an operator, shown in the menu next to its icon. The
/// trigger uses the shorter [`operator_token`] instead.
fn operator_label(op: FilterOp) -> &'static str {
    match op {
        FilterOp::Like => "Contiene",
        FilterOp::Equals => "Es igual a",
        FilterOp::In => "Está en",
        FilterOp::Gt => "Mayor que",
        FilterOp::Gte => "Mayor o igual",
        FilterOp::Lt => "Menor que",
        FilterOp::Lte => "Menor o igual",
    }
}

/// The short token shown on the trigger, so the type is readable without
/// opening the menu: `eq`, `gt`, `gte`, ...
fn operator_token(op: FilterOp) -> &'static str {
    match op {
        FilterOp::Equals => "eq",
        op => op.as_str(),
    }
}

/// Operator glyph. A single component keeps the trigger and the menu item in
/// step: both ask for the same operator and get the same picture.
#[component]
fn OperatorIcon(op: FilterOp) -> impl IntoView {
    match op {
        FilterOp::Like => view! { <CaseSensitive class="size-4" /> }.into_any(),
        FilterOp::Equals => view! { <Equal class="size-4" /> }.into_any(),
        FilterOp::In => view! { <ListIcon class="size-4" /> }.into_any(),
        FilterOp::Gt => view! { <ChevronUp class="size-4" /> }.into_any(),
        FilterOp::Gte => view! { <ChevronsUp class="size-4" /> }.into_any(),
        FilterOp::Lt => view! { <ChevronDown class="size-4" /> }.into_any(),
        FilterOp::Lte => view! { <ChevronsDown class="size-4" /> }.into_any(),
    }
}

/// The operator selector: a button showing the current operator and a menu with
/// every operator the field allows.
///
/// Rendered as a bare `DropdownMenu` so the caller can decide whether it lives
/// inside a `ButtonGroup` (which needs a `display: contents` wrapper) and
/// whether a click has to stop before reaching a parent dropdown.
#[component]
pub(super) fn OperatorPicker(
    #[prop(into)] fieldtype: String,
    operator: FilterOp,
    #[prop(into, optional)] trigger_class: String,
    on_pick: Callback<FilterOp>,
) -> impl IntoView {
    let operators = operators_for(&fieldtype);

    view! {
        <DropdownMenu align=DropdownMenuAlign::Start>
            <DropdownMenuTrigger as_child=true>
                <Button
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    // A fixed width keeps the trigger from resizing between
                    // tokens (`eq`, `gt`, `gte`, `like`...); without it every
                    // operator change moved the value input next to it. The
                    // caller only adjusts the corners so the control fits the
                    // group it lives in.
                    class=tw_merge!("w-24 justify-between", trigger_class)
                    attr:aria-label=format!(
                        "Tipo de filtro: {}",
                        operator_label(operator)
                    )
                    attr:title=operator_label(operator)
                >
                    <OperatorIcon op=operator />
                    <span class="text-xs font-medium text-muted-foreground">
                        {operator_token(operator)}
                    </span>
                    <ChevronDown class="size-3.5 shrink-0 text-muted-foreground" />
                </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent class="w-52">
                <DropdownMenuGroup>
                    {operators
                        .into_iter()
                        .map(|op| {
                            let selected = op == operator;
                            let on_click = Callback::new(move |_| on_pick.run(op));
                            view! {
                                <DropdownMenuItem>
                                    <DropdownMenuAction
                                        class="justify-between"
                                        on_click=on_click
                                    >
                                        <span class="inline-flex min-w-0 items-center gap-2">
                                            <OperatorIcon op=op />
                                            <span class="truncate">
                                                {operator_label(op)}
                                            </span>
                                        </span>
                                        {if selected {
                                            view! {
                                                <Check class="size-4 shrink-0 text-primary" />
                                            }
                                                .into_any()
                                        } else {
                                            view! {
                                                <span class="size-4 shrink-0"></span>
                                            }
                                                .into_any()
                                        }}
                                    </DropdownMenuAction>
                                </DropdownMenuItem>
                            }
                        })
                        .collect_view()}
                </DropdownMenuGroup>
            </DropdownMenuContent>
        </DropdownMenu>
    }
}

/// The value half of a filter row, switched on the field's type.
///
/// `on_commit` runs once the value is complete (a `change` event for text and
/// dates, a selection for native selects), never per keystroke. The advanced
/// dropdown leaves it unset because it stages the value until "Aplicar filtros";
/// the quick-filter row uses it to apply immediately.
#[component]
pub(super) fn FilterValueEditor(
    #[prop(into)] fieldtype: String,
    /// Newline-separated choices. Empty for a field that has none.
    #[prop(into, optional)]
    options: String,
    value: RwSignal<String>,
    #[prop(into, optional)] id: String,
    #[prop(into, optional)] class: String,
    #[prop(into, optional)] placeholder: String,
    #[prop(into, optional)] aria_label: String,
    #[prop(optional)] on_commit: Option<Callback<()>>,
) -> impl IntoView {
    let commit = on_commit;

    // A `Link` field's `options` is the target doctype, not a list of choices:
    // it must be a text input. Only `Select` carries newline-separated values.
    if fieldtype == "Select" {
        let opts: Vec<String> = options
            .split('\n')
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect();
        let on_change = Callback::new(move |ev: leptos::ev::Event| {
            value.set(event_target_value(&ev));
            if let Some(cb) = commit {
                cb.run(());
            }
        });
        view! {
            <SelectNative
                id=id
                aria_label=aria_label
                value=value.read_only()
                on_change=on_change
                class=class
            >
                <option value="">"Seleccionar..."</option>
                {opts
                    .into_iter()
                    .map(|option| {
                        let label = option.clone();
                        view! { <option value=option>{label}</option> }
                    })
                    .collect_view()}
            </SelectNative>
        }
        .into_any()
    } else if fieldtype == "Check" {
        let on_change = Callback::new(move |ev: leptos::ev::Event| {
            value.set(event_target_value(&ev));
            if let Some(cb) = commit {
                cb.run(());
            }
        });
        view! {
            <SelectNative
                id=id
                aria_label=aria_label
                value=value.read_only()
                on_change=on_change
                class=class
            >
                <option value="">"Seleccionar..."</option>
                <option value="true">"Sí"</option>
                <option value="false">"No"</option>
            </SelectNative>
        }
        .into_any()
    } else if fieldtype == "Date" || fieldtype == "Datetime" {
        let input_type = if fieldtype == "Datetime" {
            InputType::DatetimeLocal
        } else {
            InputType::Date
        };
        view! {
            <Input
                id=id
                r#type=input_type
                aria_label=aria_label
                bind_value=value
                class=class
                on:change=move |_| {
                    if let Some(cb) = commit {
                        cb.run(());
                    }
                }
            />
        }
        .into_any()
    } else {
        view! {
            <Input
                id=id
                aria_label=aria_label
                bind_value=value
                placeholder=placeholder
                class=class
                on:change=move |_| {
                    if let Some(cb) = commit {
                        cb.run(());
                    }
                }
            />
        }
        .into_any()
    }
}
