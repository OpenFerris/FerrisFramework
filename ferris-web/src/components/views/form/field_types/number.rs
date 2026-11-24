//! Numeric fields: `Int`, `Float`, `Duration`, `Currency` and `Percent`.
//!
//! They are **text** inputs with `inputmode`, not `<input type="number">`:
//! a number input reports an empty value while the text is a valid prefix
//! (`12.` or `12,`), which reset the field to empty on the first decimal
//! keystroke and made decimals impossible to type. The widget now keeps the raw
//! text, ignores characters the fieldtype cannot hold (an `Int` drops the
//! separator instead of losing the number), and parsing happens on the wire
//! side (`validate_form` / `build_form_payload`), where a comma and a dot are
//! both accepted.

use leptos::prelude::*;

use crate::components::ui::input::{Input, InputType};
use crate::components::ui::input_group::{InputGroup, InputGroupAddon, InputGroupAddonAlign};

/// The wrapper classes an `InputGroup`'s control needs (same literal as
/// `InputGroupInput`, see `number`'s sibling note).
const GROUP_CONTROL: &str = "h-9 max-md:h-10 w-full min-w-0 rounded-none border-0 bg-transparent px-3 py-1 tabular-nums outline-none";

/// Plain numeric input (`Int`, `Float`, `Duration`).
#[component]
pub fn NumberField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
    min: String,
    max: String,
    step: String,
    #[prop(optional, into)] aria_invalid: Option<Signal<bool>>,
    #[prop(into, optional)] aria_describedby: Option<String>,
    #[prop(optional)] required: bool,
) -> impl IntoView {
    // `step = "1"` is how the renderer marks an `Int`: no separators at all.
    let allow_decimal = step != "1";
    view! {
        <NumberInput
            id=id
            value=value
            read_only=read_only
            placeholder=placeholder
            min=min
            max=max
            allow_decimal=allow_decimal
            class="tabular-nums".to_string()
            aria_invalid=Signal::derive(move || aria_invalid.is_some_and(|state| state.get()))
            aria_describedby=aria_describedby.clone().unwrap_or_default()
            required=required
        />
    }
}

/// Money input (`Currency`): a number with a fixed currency prefix.
#[component]
pub fn CurrencyField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
    min: String,
    max: String,
    prefix: String,
    precision: Option<u8>,
    #[prop(optional, into)] aria_invalid: Option<Signal<bool>>,
    #[prop(into, optional)] aria_describedby: Option<String>,
    #[prop(optional)] required: bool,
) -> impl IntoView {
    let _ = precision;
    view! {
        <InputGroup>
            <InputGroupAddon align=InputGroupAddonAlign::InlineStart>{prefix}</InputGroupAddon>
            <NumberInput
                id=id
                value=value
                read_only=read_only
                placeholder=placeholder
                min=min
                max=max
                allow_decimal=true
                class=GROUP_CONTROL.to_string()
                aria_invalid=Signal::derive(move || aria_invalid.is_some_and(|state| state.get()))
                aria_describedby=aria_describedby.clone().unwrap_or_default()
                required=required
            />
        </InputGroup>
    }
}

/// Percentage input (`Percent`): a number with a trailing `%`.
#[component]
pub fn PercentField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
    min: String,
    max: String,
    precision: Option<u8>,
    #[prop(optional, into)] aria_invalid: Option<Signal<bool>>,
    #[prop(into, optional)] aria_describedby: Option<String>,
    #[prop(optional)] required: bool,
) -> impl IntoView {
    let _ = precision;
    view! {
        <InputGroup>
            <InputGroupAddon align=InputGroupAddonAlign::InlineEnd>"%"</InputGroupAddon>
            <NumberInput
                id=id
                value=value
                read_only=read_only
                placeholder=placeholder
                min=min
                max=max
                allow_decimal=true
                class=GROUP_CONTROL.to_string()
                aria_invalid=Signal::derive(move || aria_invalid.is_some_and(|state| state.get()))
                aria_describedby=aria_describedby.clone().unwrap_or_default()
                required=required
            />
        </InputGroup>
    }
}

/// The shared control: a text input that keeps only what the field can store.
#[component]
fn NumberInput(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
    min: String,
    max: String,
    allow_decimal: bool,
    class: String,
    #[prop(optional, into)] aria_invalid: Option<Signal<bool>>,
    #[prop(into, optional)] aria_describedby: Option<String>,
    #[prop(optional)] required: bool,
) -> impl IntoView {
    let _ = (min, max);
    let on_input = Callback::new(move |raw: String| {
        // Always write, even when the filtered text equals the stored one: the
        // write re-renders `prop:value` and that is what removes the rejected
        // character from the DOM (typing `.` in an `Int` must not leave it on
        // screen while the value is empty).
        value.set(sanitize_number(&raw, allow_decimal));
    });
    view! {
        <Input
            id=id
            r#type=InputType::Text
            attr:inputmode=if allow_decimal { "decimal" } else { "numeric" }
            autocomplete="off"
            readonly=read_only
            placeholder=placeholder
            prop:value=move || value.get()
            on_input=on_input
            class=class
            aria_invalid=Signal::derive(move || aria_invalid.is_some_and(|state| state.get()))
            aria_describedby=aria_describedby.clone().unwrap_or_default()
            required=required
        />
    }
}

/// Keep digits, an optional leading sign and — when the field allows it — a
/// single decimal separator (`.` or the locale's `,`). Everything else is
/// ignored, which is what "a field without decimals ignores the separator"
/// means in practice.
fn sanitize_number(raw: &str, allow_decimal: bool) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut separator = false;
    for ch in raw.chars() {
        if ch.is_ascii_digit() {
            out.push(ch);
        } else if (ch == '-' || ch == '+') && out.is_empty() {
            out.push(ch);
        } else if (ch == '.' || ch == ',') && allow_decimal && !separator {
            separator = true;
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sanitize_number;

    #[test]
    fn an_int_ignores_the_decimal_separator() {
        assert_eq!(sanitize_number("12.3", false), "123");
        assert_eq!(sanitize_number("12,3", false), "123");
        assert_eq!(sanitize_number("12..3", false), "123");
        assert_eq!(sanitize_number("-5", false), "-5");
    }

    #[test]
    fn a_decimal_field_keeps_one_separator() {
        assert_eq!(sanitize_number("12.34", true), "12.34");
        assert_eq!(sanitize_number("12,34", true), "12,34");
        assert_eq!(sanitize_number("12..34", true), "12.34");
        assert_eq!(sanitize_number("1.2,3", true), "1.23");
        assert_eq!(sanitize_number("12.", true), "12.");
        assert_eq!(sanitize_number("abc12x", true), "12");
    }
}
