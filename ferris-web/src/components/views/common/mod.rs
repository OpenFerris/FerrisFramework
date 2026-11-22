//! Reusable list view components.

use leptos::prelude::*;

use crate::components::ui::skeleton::Skeleton;

/// Loading skeleton for list views.
#[component]
pub fn ListLoadingSkeleton(row_count: usize) -> impl IntoView {
    view! {
        <div class="flex w-full flex-1 flex-col gap-2 rounded-xl border border-border bg-card p-4 sm:p-6">
            {(0..row_count).map(|_| view! { <Skeleton class="h-9 w-full" /> }).collect_view()}
        </div>
    }
}

/// Error banner for list views.
#[component]
pub fn ListErrorBanner(#[prop(into)] error: Signal<Option<String>>) -> impl IntoView {
    view! {
        <Show when=move || error.get().is_some()>
            <div
                class="flex items-start gap-2 rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
                role="alert"
            >
                <span class="min-w-0 break-words">{move || error.get().unwrap_or_default()}</span>
            </div>
        </Show>
    }
}
