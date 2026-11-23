//! Order-by group: a direction toggle next to the dropdown listing every
//! sortable field.

use leptos::prelude::*;

use crate::api::meta::Column;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::button_group::ButtonGroup;
use crate::components::ui::dropdown_menu::{
    DropdownMenu, DropdownMenuAction, DropdownMenuAlign, DropdownMenuContent, DropdownMenuGroup,
    DropdownMenuItem, DropdownMenuTrigger,
};
use crate::i18n::t;

use heck::ToUpperCamelCase;
use icons::{ArrowDownWideNarrow, ArrowUpNarrowWide, Check, ChevronDown};

#[component]
pub fn ListOrderByGroup(
    #[prop(into)] sort_state: Signal<(String, bool)>,
    #[prop(into)] fields: Signal<Vec<Column>>,
    on_set: Callback<(String, bool)>,
) -> impl IntoView {
    // Label of the field the list is currently ordered by.
    let field_label = move || {
        let (field, _) = sort_state.get();
        fields
            .get()
            .iter()
            .find(|c| c.fieldname == field)
            .map(|c| c.label.to_upper_camel_case())
            .unwrap_or_else(|| field.to_upper_camel_case())
    };

    view! {
        <ButtonGroup class="min-w-0 max-w-full">
            // Botón para cambiar la dirección (Ascendente / Descendente)
            <Button
                variant=ButtonVariant::Outline
                size=ButtonSize::Sm
                attr:aria-label=t("components.list.order_by.toggle_direction")
                on:click=move |_| {
                    let (field, desc) = sort_state.get_untracked();
                    on_set.run((field, !desc));
                }
            >
                {move || {
                    let is_desc = sort_state.get().1;
                    if is_desc {
                        view! { <ArrowDownWideNarrow class="size-4" /> }.into_any()
                    } else {
                        view! { <ArrowUpNarrowWide class="size-4" /> }.into_any()
                    }
                }}
            </Button>

            // Desplegable de selección de campo
            <DropdownMenu align=DropdownMenuAlign::End>
                <DropdownMenuTrigger as_child=true>
                    <Button variant=ButtonVariant::Outline size=ButtonSize::Sm>
                        <span class="min-w-0 max-w-[8rem] truncate sm:max-w-none">{field_label}</span>
                        <ChevronDown class="size-4 shrink-0 text-muted-foreground" />
                    </Button>
                </DropdownMenuTrigger>

                <DropdownMenuContent class="w-56 max-w-[calc(100vw-1rem)]">
                    <DropdownMenuGroup>
                        {move || {
                            fields
                                .get()
                                .into_iter()
                                .map(|column| {
                                    let fieldname = column.fieldname.clone();
                                    let fieldname_click = column.fieldname;
                                    let label = column.label.to_upper_camel_case();

                                    view! {
                                        <DropdownMenuItem
                                            on:click=move |_| {
                                                let (_, desc) = sort_state.get_untracked();
                                                on_set.run((fieldname_click.clone(), desc));
                                            }
                                        >
                                            <DropdownMenuAction class="flex-1 justify-between">
                                                <span class="truncate">{label}</span>
                                                {move || {
                                                    let is_active = sort_state.get().0 == fieldname;
                                                    if is_active {
                                                        view! { <Check class="size-4 text-primary" /> }.into_any()
                                                    } else {
                                                        view! { <span class="size-4"></span> }.into_any()
                                                    }
                                                }}
                                            </DropdownMenuAction>
                                        </DropdownMenuItem>
                                    }
                                })
                                .collect_view()
                        }}
                    </DropdownMenuGroup>
                </DropdownMenuContent>
            </DropdownMenu>
        </ButtonGroup>
    }
}
