//! Presentational pagination built on rust-ui buttons and selects.

use leptos::prelude::*;

use crate::components::constants::pagination::PAGINATION;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::select_native::SelectNative;
use crate::i18n::use_i18n;

/// Pagination controls for list views. `current_page` is 0-based internally.
#[component]
pub fn Pagination(
    current_page: ReadSignal<u32>,
    total_pages: Memo<u32>,
    on_page_change: WriteSignal<u32>,
    page_size: RwSignal<u32>,
) -> impl IntoView {
    let i18n = use_i18n();
    let prev_disabled = move || current_page.get() == 0 || total_pages.get() <= 1;
    let next_disabled = move || current_page.get() + 1 >= total_pages.get();

    let on_prev = move |_| {
        on_page_change.update(|p| *p = p.saturating_sub(1));
    };
    let on_next = move |_| {
        on_page_change
            .update(|p| *p = p.saturating_add(1).min(total_pages.get().saturating_sub(1)));
    };

    // The picker mirrors the page size signal, which other controls may also
    // change; keeping a local `String` copy would let the two disagree.
    let size_value = RwSignal::new(page_size.get_untracked().to_string());
    Effect::new(move |_| size_value.set(page_size.get().to_string()));

    let i18n_page = i18n;
    view! {
        <div class="flex flex-wrap items-center gap-2 px-2 py-2">
            <SelectNative
                id="page_size"
                class="w-full sm:w-24"
                aria_label=i18n.t("views.common.rows_per_page")
                value=size_value.read_only()
                on_change=Callback::new(move |ev: leptos::ev::Event| {
                    let text = event_target_value(&ev);
                    let v = text.parse::<u32>().unwrap_or(PAGINATION::DEFAULT_PAGE_SIZE);
                    if v != page_size.get_untracked() {
                        page_size.set(v);
                        on_page_change.set(0);
                    }
                })
            >
                {
                    PAGINATION::PAGE_SIZE_OPTIONS.iter().map(|s| {
                        let sv = s.to_string();
                        view! { <option value={sv.clone()}>{sv.clone()}</option> }
                    }).collect_view()
                }
            </SelectNative>
            <div class="flex min-w-0 items-center gap-1 text-sm text-muted-foreground">
                <span>{move || i18n_page.t_with(
                    "views.common.page_of",
                    &[
                        ("page", &(current_page.get() + 1).to_string()),
                        ("total", &total_pages.get().to_string()),
                    ],
                )}</span>
            </div>
            <div class="ml-auto flex items-center gap-1">
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    attr:disabled=prev_disabled
                    on:click=on_prev
                >
                    {i18n.t("views.common.previous")}
                </Button>
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    attr:disabled=next_disabled
                    on:click=on_next
                >
                    {i18n.t("views.common.next")}
                </Button>
            </div>
        </div>
    }
}
