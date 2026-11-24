//! Date and time inputs.
//!
//! All three are native HTML inputs: the browser already ships a locale-aware
//! picker, keyboard entry and validation, and using it keeps the field working
//! during SSR with no client-side script. The wrapper exists so the renderer
//! has one place to route `Date`/`Datetime`/`Time` and one place to change if
//! that decision ever does.

use crate::components::ui::input::{Input, InputType};
use leptos::prelude::*;

/// Calendar date (`Date`), `YYYY-MM-DD`.
#[component]
pub fn DateField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
    #[prop(optional, into)] aria_invalid: Option<Signal<bool>>,
    #[prop(into, optional)] aria_describedby: Option<String>,
    #[prop(optional)] required: bool,
) -> impl IntoView {
    view! {
        <Input id=id r#type=InputType::Date bind_value=value readonly=read_only placeholder=placeholder
            aria_invalid=Signal::derive(move || aria_invalid.is_some_and(|state| state.get()))
            aria_describedby=aria_describedby.clone().unwrap_or_default()
            required=required
        />
    }
}

/// Local date and time (`Datetime`).
///
/// The native control speaks `YYYY-MM-DDTHH:MM[:SS]`, while the engine stores
/// RFC 3339 (offset included) or `YYYY-MM-DD HH:MM:SS`. Binding the stored
/// string straight into the input left it empty on load — the offset is
/// invalid there — and sent `…T10:30` back, which the engine rejects with
/// "Invalid Datetime format". The widget keeps a local signal in the input's
/// format and mirrors it to the field, so the wall clock the engine stores is
/// the one the user edits.
#[component]
pub fn DateTimeField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
    #[prop(optional, into)] aria_invalid: Option<Signal<bool>>,
    #[prop(into, optional)] aria_describedby: Option<String>,
    #[prop(optional)] required: bool,
) -> impl IntoView {
    let local = RwSignal::new(to_local_input(&value.get_untracked()));
    // Stored → input (a document load, a fetch_from, another widget).
    Effect::new(move |_| {
        let shown = to_local_input(&value.get());
        if local.get_untracked() != shown {
            local.set(shown);
        }
    });
    // Input → stored, so the payload always sees the edited value.
    Effect::new(move |_| {
        let current = local.get();
        if value.get_untracked() != current {
            value.set(current);
        }
    });
    view! {
        <Input
            id=id
            r#type=InputType::DatetimeLocal
            bind_value=local
            readonly=read_only
            placeholder=placeholder
            aria_invalid=Signal::derive(move || aria_invalid.is_some_and(|state| state.get()))
            aria_describedby=aria_describedby.clone().unwrap_or_default()
            required=required
        />
    }
}

/// `RFC 3339` / `YYYY-MM-DD HH:MM:SS` → the `YYYY-MM-DDTHH:MM:SS` the native
/// `datetime-local` input accepts. The offset is dropped on purpose: the Desk
/// edits the wall clock the engine stores, and converting time zones in the
/// browser would silently shift every value.
fn to_local_input(stored: &str) -> String {
    let text = stored.trim();
    if text.is_empty() {
        return String::new();
    }
    let Some((date, rest)) = text.split_once('T').or_else(|| text.split_once(' ')) else {
        return text.trim_end_matches('Z').to_string();
    };
    let time = rest.trim_end_matches('Z');
    // A numeric offset (`+02:00`, `-05:00`) is not part of the wall clock.
    let time = time.split(['+', '-']).next().unwrap_or_default();
    let mut parts = time.split(':');
    let hour = parts.next().unwrap_or("00");
    let minute = parts.next().unwrap_or("00");
    let second = parts.next().map_or("00", |s| &s[..s.len().min(2)]);
    format!("{date}T{hour}:{minute}:{second}")
}

/// Clock time (`Time`).
#[component]
pub fn TimeField(id: String, value: RwSignal<String>, read_only: bool) -> impl IntoView {
    view! {
        <Input id=id r#type=InputType::Time bind_value=value readonly=read_only />
    }
}

/// ISO week (`Week`), `YYYY-Www`, on the browser's native week picker —
/// Frappe's `Week` fieldtype.
#[component]
pub fn WeekField(id: String, value: RwSignal<String>, read_only: bool) -> impl IntoView {
    view! {
        <Input id=id r#type=InputType::Week bind_value=value readonly=read_only />
    }
}

#[cfg(test)]
mod tests {
    use super::to_local_input;

    /// Every stored shape the engine reads back must land in the native
    /// control's format, and the wall clock must survive unchanged.
    #[test]
    fn stored_datetimes_become_the_native_input_format() {
        assert_eq!(
            to_local_input("2026-10-08T10:30:00Z"),
            "2026-10-08T10:30:00"
        );
        assert_eq!(
            to_local_input("2026-10-08T10:30:00+02:00"),
            "2026-10-08T10:30:00"
        );
        assert_eq!(
            to_local_input("2026-10-08T10:30:00-05:00"),
            "2026-10-08T10:30:00"
        );
        assert_eq!(to_local_input("2026-10-08 10:30:00"), "2026-10-08T10:30:00");
        assert_eq!(to_local_input("2026-10-08T10:30"), "2026-10-08T10:30:00");
        assert_eq!(to_local_input(""), "");
    }
}
