//! Empty-body state shared by the filtered and unfiltered list states.

use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::icon::Icon;
use crate::i18n::{t, use_i18n};
use leptos::prelude::*;

/// The body's empty state, rendered full-width under the table header.
///
/// It used to be a `colspan` cell inside a `min-w-[720px]` table: on a phone the
/// text was centred in the 720px grid and most of it sat off-screen. A block
/// outside the table keeps the header (and its columns) while the message fits
/// the viewport. Both empty states share it so they cannot drift apart.
#[component]
pub(super) fn ListEmptyBlock(
    #[prop(into)] filtered: Signal<bool>,
    #[prop(into)] can_create: Signal<bool>,
    on_create: Callback<()>,
    on_reset_filters: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="flex min-h-40 flex-col items-center justify-center gap-3 px-4 py-8 text-center text-muted-foreground">
            <span class="flex size-10 items-center justify-center rounded-lg bg-muted text-muted-foreground">
                <Icon name="Inbox" class="size-5" />
            </span>
            <span class="text-sm">
                {move || {
                    if filtered.get() {
                        i18n.t("components.list.empty.filtered")
                    } else {
                        i18n.t("components.list.empty.none")
                    }
                }}
            </span>
            <div class="flex flex-wrap items-center justify-center gap-2">
                {move || {
                    can_create.get().then(|| {
                        view! {
                            <Button
                                variant=ButtonVariant::Default
                                size=ButtonSize::Sm
                                on:click=move |_| on_create.run(())
                            >
                                <Icon name="Plus" class="size-4" />
                                {t("components.list.empty.create")}
                            </Button>
                        }
                    })
                }}
                <Show when=move || filtered.get()>
                    <Button
                        variant=ButtonVariant::Outline
                        size=ButtonSize::Sm
                        on:click=move |_| on_reset_filters.run(())
                    >
                        <Icon name="FilterX" class="size-4" />
                        {t("components.list.empty.clear_filters")}
                    </Button>
                </Show>
            </div>
        </div>
    }
}
