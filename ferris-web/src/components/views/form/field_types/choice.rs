//! Discrete-choice inputs: a dropdown, a boolean and a rating.

use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::select_native::SelectNative;
use crate::i18n::{t, use_i18n};
use leptos::prelude::*;

/// Single-choice field (`Select`).
///
/// The first option is an empty one so a not-yet-chosen value is representable
/// and the user can clear a choice; `reqd` is enforced by `validate_form`, not
/// by the widget.
#[component]
pub fn SelectField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    options: String,
) -> impl IntoView {
    view! {
        <SelectNative
            id=id
            value=value.read_only()
            attr:disabled=read_only
            on_change=Callback::new(move |ev: leptos::ev::Event| {
                value.set(event_target_value(&ev));
            })
        >
            <option value="">""</option>
            {options
                .split('\n')
                .filter(|opt| !opt.trim().is_empty())
                .map(|opt| {
                    let o = opt.trim().to_string();
                    view! { <option value={o.clone()}>{o.clone()}</option> }
                })
                .collect_view()}
        </SelectNative>
    }
}

/// Boolean field (`Check`).
///
/// `Checkbox` renders its own button, so the control has no `<input>`; the
/// `id` is forwarded to that button (a labelable element) so the shared
/// `<label for=…>` chrome still focuses and toggles it.
#[component]
pub fn CheckField(
    id: String,
    checked: RwSignal<bool>,
    read_only: bool,
    label: String,
) -> impl IntoView {
    view! {
        <Checkbox
            attr:id=id
            checked=Signal::derive(move || checked.get())
            on_checked_change=Callback::new(move |b| checked.set(b))
            aria_label=if label.is_empty() {
                t("components.form.field.check_label")
            } else {
                label
            }
            disabled=Signal::derive(move || read_only)
        />
    }
}

/// Star rating (`Rating`, 1–5). Stored as text, like every other field value.
#[component]
pub fn RatingField(id: String, value: RwSignal<String>, read_only: bool) -> impl IntoView {
    let i18n = use_i18n();
    let current = Signal::derive(move || value.get().parse::<u8>().unwrap_or(0));
    view! {
        <div id=id class="flex items-center gap-0.5" role="radiogroup" aria-label=i18n.t("components.form.field.rating")>
            {(1u8..=5)
                .map(|star| {
                    let filled = Signal::derive(move || current.get() >= star);
                    let star_label = i18n.t_with(
                        "components.form.field.rating_star",
                        &[("star", &star.to_string())],
                    );
                    view! {
                        <button
                            type="button"
                            role="radio"
                            aria-checked=move || (current.get() == star).to_string()
                            aria-label=star_label
                            disabled=read_only
                            // `p-0.5` around a glyph is a ~24px target; on a
                            // phone each star becomes a 40px square so the row
                            // is usable with a thumb. Desktop keeps the size.
                            class="rounded-sm p-0.5 transition-colors hover:bg-accent disabled:cursor-not-allowed max-md:flex max-md:min-h-10 max-md:min-w-10 max-md:items-center max-md:justify-center"
                            on:click=move |_| value.set(star.to_string())
                        >
                            <span class=move || {
                                if filled.get() { "text-warning" } else { "text-muted-foreground" }
                            }>{move || if filled.get() { "★" } else { "☆" }}</span>
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}
