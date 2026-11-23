//! The small property controls of the field inspector.
//!
//! They live here rather than in `field.rs` because they are pure presentation
//! of one value: splitting them keeps the inspector (the state machine that
//! diffs a field against its overrides) readable on its own.

use leptos::prelude::*;

use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::icon::Icon;
use crate::components::ui::input::{Input, InputType};
use crate::components::ui::popover::{Popover, PopoverAlign, PopoverContent, PopoverTrigger};
use crate::components::ui::textarea::Textarea;
use crate::i18n::{t, use_i18n};

/// A text property: label, optional "editado" marker, input or textarea.
#[component]
pub(super) fn PropText(
    title: String,
    overridden: bool,
    value: RwSignal<String>,
    #[prop(optional)] multiline: bool,
    #[prop(optional)] hint: String,
    /// A worked example of the expected expression, shown in the info tooltip.
    /// Used by the expression-like properties (visibility, ordering), where a
    /// bare label does not tell the user what to type.
    #[prop(optional)]
    example: Option<String>,
) -> impl IntoView {
    // `StoredValue` is `Copy`, so the same id can be read from the label and
    // from both branches of the control without being moved into one closure.
    let id = StoredValue::new(control_id(&title));
    let hint_view = hint.clone();
    let hint_for_show = hint.clone();
    view! {
        <div class="flex flex-col gap-1">
            <div class="flex items-center justify-between gap-2">
                <span class="flex items-center gap-1">
                    <label class="text-xs font-medium text-muted-foreground" for=id.get_value()>
                        {title.clone()}
                    </label>
                    {example.map(|example| view! { <InfoHint title=title.clone() example=example /> })}
                </span>
                <Show when=move || overridden>
                    <span class="rounded bg-secondary px-1.5 py-0.5 text-xs text-secondary-foreground">
                        {t("components.form.edit_mode.edited")}
                    </span>
                </Show>
            </div>
            <Show
                when=move || multiline
                fallback=move || view! {
                    <Input
                        id=id.get_value()
                        r#type=InputType::Text
                        prop:value=move || value.get()
                        on_input=Callback::new(move |text: String| value.set(text))
                    />
                }
            >
                <Textarea id=id.get_value() bind_value=value rows=2u32 />
            </Show>
            <Show when=move || !hint_for_show.is_empty()>
                <p class="text-xs text-muted-foreground">{hint_view.clone()}</p>
            </Show>
        </div>
    }
}

/// A boolean property: label, "editado" marker and a checkbox.
#[component]
pub(super) fn PropCheck(title: String, overridden: bool, value: RwSignal<bool>) -> impl IntoView {
    let aria_label = title.clone();
    view! {
        <div class="flex items-center justify-between gap-3">
            <span class="flex items-center gap-2 text-sm">
                {title}
                <Show when=move || overridden>
                    <span class="rounded bg-secondary px-1.5 py-0.5 text-xs text-secondary-foreground">
                        {t("components.form.edit_mode.edited")}
                    </span>
                </Show>
            </span>
            <Checkbox
                checked=Signal::derive(move || value.get())
                aria_label=aria_label
                on_checked_change=Callback::new(move |checked: bool| value.set(checked))
            />
        </div>
    }
}

/// The info affordance of an expression-like property.
///
/// A real `<button>` (not a decorative span) so the example is reachable with
/// the keyboard, and a **popover** — not a CSS tooltip — for the content: the
/// properties panel is a scroll pane inside an 18rem column, and an absolutely
/// positioned bubble there is clipped by the pane (and, because it overflows
/// horizontally too, made the sidebar grow scrollbars). A popover renders in
/// the browser's top layer and is positioned against its anchor, so the hint
/// can never push the panel's scroll area.
///
/// It opens on click (and Escape closes it), which also survives touch, where a
/// hover tooltip never appears.
#[component]
fn InfoHint(title: String, example: String) -> impl IntoView {
    let i18n = use_i18n();
    let aria = i18n.t_with(
        "components.form.edit_mode.example_for",
        &[("title", &title), ("example", &example)],
    );
    let tooltip = i18n.t_with(
        "components.form.edit_mode.example_tooltip",
        &[("example", &example)],
    );
    view! {
        <Popover align=PopoverAlign::StartOuter>
            <PopoverTrigger
                class="size-4 shrink-0 rounded-full border-0 bg-transparent p-0 text-muted-foreground hover:bg-accent hover:text-foreground max-md:size-4"
                attr:aria-label=aria
                attr:title=tooltip
            >
                <Icon name="circle-help" class="size-3.5" />
            </PopoverTrigger>
            <PopoverContent class="min-h-0 w-auto max-w-[18rem] p-2.5 text-xs shadow-lg">
                <p class="mb-1 text-xs font-medium text-muted-foreground">
                    {t("components.form.edit_mode.example")}
                </p>
                <code class="block break-words rounded bg-muted px-1.5 py-1 font-mono text-xs text-foreground">
                    {example}
                </code>
            </PopoverContent>
        </Popover>
    }
}

/// A DOM id from a property title (`Valor por defecto` → `prop-valor-por-defecto`).
///
/// The title is Spanish UI text with spaces and accents; an `id`/`for` pair with
/// a space in it is invalid HTML and the label would not focus the control.
fn control_id(title: &str) -> String {
    let mut out = String::from("prop-");
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    out
}
