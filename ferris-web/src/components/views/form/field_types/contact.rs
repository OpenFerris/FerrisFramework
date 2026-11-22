//! Contact and address inputs: email, phone and URL.

use crate::components::ui::input::{Input, InputType};
use crate::components::ui::input_phone::InputPhone;
use crate::components::utils::country::Country;
use crate::components::utils::phone_number::PhoneNumber;
use leptos::prelude::*;

/// Email address (`Email`): the native `email` input, so the browser offers its
/// own keyboard and validation without a custom widget.
#[component]
pub fn EmailField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
) -> impl IntoView {
    view! {
        <Input id=id r#type=InputType::Email bind_value=value readonly=read_only placeholder=placeholder />
    }
}

/// Phone number (`Phone`): the dedicated `InputPhone` widget, which owns the
/// country selector and formatting. The stored value is kept in international
/// form, so the field round-trips between the document and the widget.
///
/// Loading parses the stored value back into subscriber digits (stripping the
/// dial code it was formatted with) and resolves the country from that prefix.
/// Keeping the dial code in the digits made every load prepend another one and
/// silently corrupted the stored number (WS5).
#[component]
pub fn PhoneField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    // Read once to seed the widget: it is the initial value, not a dependency
    // of this render (the `Effect` below keeps the two in step afterwards).
    let (loaded, loaded_country) = PhoneNumber::parse(&value.get_untracked(), Country::Spain);
    let phone = RwSignal::new(loaded);
    let country = RwSignal::new(loaded_country);
    let phone_error = error;

    Effect::new(move |_| {
        let v = value.get();
        let saved = {
            let ph = phone.get();
            if ph.is_empty() {
                String::new()
            } else {
                ph.format_international(country.get())
            }
        };
        if v != saved {
            value.set(saved);
        }
    });

    view! {
        <InputPhone
            id=id
            value_signal=phone
            country_signal=country
            disabled=read_only
            invalid=Signal::derive(move || phone_error.get().is_some())
        />
    }
}

/// URL (`URL`): the native `url` input.
#[component]
pub fn UrlField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
) -> impl IntoView {
    view! {
        <Input id=id r#type=InputType::Url bind_value=value readonly=read_only placeholder=placeholder />
    }
}
