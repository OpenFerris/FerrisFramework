//! Column chooser of the list view.
//!
//! The service has persisted a visible-column set since the list was built
//! (`ListService::toggle_column`), but nothing ever called it: the only way to
//! change columns was the metadata. This menu is that caller, so the decision
//! "which metadata columns do I want in front of me" is the user's, stored in
//! the same `SavedView` as the filters and the ordering.
//!
//! The menu stays open while columns are toggled (`DropdownMenuCustomItem`
//! carries no close marker): choosing several is one visit, not five.

use leptos::prelude::*;

use crate::api::meta::Column;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::dropdown_menu::{
    DropdownMenu, DropdownMenuAction, DropdownMenuAlign, DropdownMenuContent,
    DropdownMenuCustomItem, DropdownMenuGroup, DropdownMenuSeparator, DropdownMenuTrigger,
};
use crate::components::ui::icon::Icon;
use crate::i18n::t;

/// A menu of every column the doctype offers, with one checkbox each.
#[component]
pub fn ListColumnPicker(
    /// Every column the list could show.
    #[prop(into)]
    columns: Signal<Vec<Column>>,
    /// The columns it draws now (the effective set, never empty).
    #[prop(into)]
    visible: Signal<Vec<Column>>,
    /// `(fieldname, checked)`.
    on_toggle: Callback<(String, bool)>,
    on_reset: Callback<()>,
) -> impl IntoView {
    view! {
        <DropdownMenu align=DropdownMenuAlign::End>
            <DropdownMenuTrigger as_child=true>
                <Button
                    variant=ButtonVariant::Outline
                    size=ButtonSize::IconSm
                    attr:aria-label=t("components.list.column_picker.aria")
                    attr:title=t("components.list.column_picker.title")
                >
                    <Icon name="Columns3" class="size-4" />
                </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent>
                <DropdownMenuGroup>
                    <li class="px-2 py-1.5 text-xs font-medium text-muted-foreground">
                        {t("components.list.column_picker.visible")}
                    </li>
                    {move || {
                        columns
                            .get()
                            .into_iter()
                            .map(|column| {
                                let fieldname = column.fieldname;
                                let fieldname_for_check = fieldname.clone();
                                let label = column.label;
                                let checked = Signal::derive(move || {
                                    visible.with(|cols| {
                                        cols.iter()
                                            .any(|candidate| candidate.fieldname == fieldname_for_check)
                                    })
                                });
                                view! {
                                    <DropdownMenuCustomItem>
                                        <Checkbox
                                            aria_label=label.clone()
                                            checked=checked
                                            on_checked_change=Callback::new(move |on: bool| {
                                                on_toggle.run((fieldname.clone(), on));
                                            })
                                        />
                                        <span class="min-w-0 flex-1 truncate">{label}</span>
                                    </DropdownMenuCustomItem>
                                }
                            })
                            .collect_view()
                    }}
                </DropdownMenuGroup>
                <DropdownMenuSeparator />
                <DropdownMenuAction on_click=on_reset>
                    {t("components.list.column_picker.reset")}
                </DropdownMenuAction>
            </DropdownMenuContent>
        </DropdownMenu>
    }
}
