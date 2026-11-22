//! The row that opens one sidebar section.

use leptos::prelude::*;

use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::icon::Icon;

/// One section row: icon + label on the left, a `+` on the right.
///
/// The row's label is not the control: the small button is, so the affordance is
/// the same for Assign (opens a picker), Attachments (opens the file dialog) and
/// Tags (opens the search box) — one behaviour to learn, not four.
#[component]
pub(super) fn SidebarRow(
    icon: &'static str,
    label: &'static str,
    aria_label: &'static str,
    on_add: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="flex items-center justify-between gap-2">
            <span class="flex items-center gap-2 text-sm font-medium text-foreground">
                <Icon name=icon class="size-4 text-muted-foreground" />
                {label}
            </span>
            <Button
                variant=ButtonVariant::Ghost
                size=ButtonSize::IconSm
                class="size-7 text-muted-foreground hover:text-foreground"
                attr:aria-label=aria_label
                attr:title=aria_label
                on:click=move |_| on_add.run(())
            >
                <Icon name="plus" class="size-4" />
            </Button>
        </div>
    }
}
