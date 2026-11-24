//! Text-family inputs: prose and code.
//!
//! `Text`/`Small Text`/`Long Text` share the multi-line [`Textarea`]; `Code`
//! and `HTML Editor` reuse it with a monospace face rather than a second
//! widget. The rich editors are not here: `Text Editor` (Quill) and
//! `Markdown Editor` (marked) are their own widgets in [`super::editor`],
//! because they replace the textarea instead of restyling it. One component
//! per *behaviour*, not per fieldtype, is what keeps the renderer's match and
//! the visual result aligned.

use crate::components::ui::textarea::Textarea;
use leptos::prelude::*;

/// Multi-line text (`Text`, `Small Text`, `Long Text`).
#[component]
pub fn TextField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
    rows: u32,
    /// `max_length` from the field metadata; omitted when the doctype sets none.
    max_length: Option<u32>,
    #[prop(optional, into)] aria_invalid: Option<Signal<bool>>,
    #[prop(into, optional)] aria_describedby: Option<String>,
    #[prop(optional)] required: bool,
) -> impl IntoView {
    view! {
        <Textarea
            id=id
            bind_value=value
            readonly=read_only
            placeholder=placeholder
            rows=rows
            attr:maxlength=max_length
            aria_invalid=Signal::derive(move || aria_invalid.is_some_and(|state| state.get()))
            aria_describedby=aria_describedby.clone().unwrap_or_default()
            required=required
        />
    }
}

/// Source-like text (`Code`, `HTML Editor`): the same textarea with a
/// monospace, slightly smaller face so indentation and line breaks are legible.
///
/// The face stays at 16px on a phone (`max-md:text-base`): a smaller one makes
/// iOS Safari zoom the page when the field takes focus, which throws the whole
/// form layout off.
#[component]
pub fn CodeField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
    rows: u32,
    max_length: Option<u32>,
    #[prop(optional, into)] aria_invalid: Option<Signal<bool>>,
    #[prop(into, optional)] aria_describedby: Option<String>,
    #[prop(optional)] required: bool,
) -> impl IntoView {
    view! {
        <Textarea
            id=id
            bind_value=value
            readonly=read_only
            placeholder=placeholder
            rows=rows
            class="font-mono text-xs max-md:text-base"
            attr:maxlength=max_length
            aria_invalid=Signal::derive(move || aria_invalid.is_some_and(|state| state.get()))
            aria_describedby=aria_describedby.clone().unwrap_or_default()
            required=required
        />
    }
}
