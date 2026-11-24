//! Colour input.
//!
//! Uses the native `color` input, which already gives a swatch and a picker on
//! every platform. `readonly` is meaningless on `<input type="color">` — the
//! browser ignores it — so a read-only field uses `disabled` instead, which
//! genuinely blocks the picker and the value.

use crate::components::ui::input::{Input, InputType};
use leptos::prelude::*;

/// Colour (`Color`), stored as `#rrggbb`.
#[component]
pub fn ColorField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    #[prop(optional, into)] aria_invalid: Option<Signal<bool>>,
    #[prop(into, optional)] aria_describedby: Option<String>,
    #[prop(optional)] required: bool,
) -> impl IntoView {
    view! {
        <div class="flex min-w-0 items-center gap-2">
            <Input
                id=id
                r#type=InputType::Color
                bind_value=value
                disabled=read_only
                class="h-9 w-14 shrink-0 cursor-pointer p-1"
            aria_invalid=Signal::derive(move || aria_invalid.is_some_and(|state| state.get()))
            aria_describedby=aria_describedby.clone().unwrap_or_default()
            required=required

            />
            <span class="min-w-0 truncate font-mono text-xs text-muted-foreground">{move || value.get()}</span>
        </div>
    }
}
