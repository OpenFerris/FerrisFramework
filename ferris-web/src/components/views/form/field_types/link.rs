//! `Link` and `Table MultiSelect` fields.
//!
//! Both are thin adapters over [`DocPicker`], the universal document picker:
//! the debounce, the accessible listbox, the keyboard cursor and the labels live
//! there once. This module only translates the field's wire shape — a single
//! name for `Link`, a JSON array of names for `Table MultiSelect` — into the
//! picker's `Vec<String>` and back.

use leptos::prelude::*;

use crate::components::pickers::{DocPicker, PickerMode};
use crate::i18n::t;
use crate::views::services::parse_name_list;

/// A `Link` field: one document name, picked from `target`.
///
/// An empty `target` (a `Dynamic Link` whose target lives in another field)
/// keeps the input usable as typed text but never searches.
///
/// `on_pick`, when set, turns the widget into a **pure search box**: choosing an
/// option calls the callback with the picked name and clears the input, instead
/// of writing `value`.
#[component]
pub fn LinkFieldComponent(
    id: String,
    value: RwSignal<String>,
    placeholder: String,
    read_only: bool,
    target: String,
    #[prop(optional, into)] on_pick: Option<Callback<String>>,
) -> impl IntoView {
    let selected = RwSignal::new(vec_or_empty(&value.get_untracked()));

    // External changes (document load, a `fetch_from`, another widget) flow in.
    Effect::new(move |_| {
        let current = value.get();
        let expected = selected
            .get_untracked()
            .first()
            .cloned()
            .unwrap_or_default();
        if current != expected {
            selected.set(vec_or_empty(&current));
        }
    });

    let on_change = Callback::new(move |next: Vec<String>| {
        let name = next.first().cloned().unwrap_or_default();
        if let Some(callback) = on_pick
            && !name.is_empty()
        {
            callback.run(name);
            // The pure-search mode empties the box for the next query.
            value.set(String::new());
            selected.set(Vec::new());
            return;
        }
        value.set(name);
    });

    view! {
        <DocPicker
            id=id
            target=target
            values=selected
            mode=PickerMode::Single
            placeholder=placeholder
            read_only=read_only
            on_change=on_change
        />
    }
}

/// A `Table MultiSelect`: several document names from `target`.
///
/// The widget stores a JSON array of names in the field's value, which is
/// exactly what the payload sends and what validation checks.
#[component]
pub fn MultiLinkField(
    id: String,
    value: RwSignal<String>,
    placeholder: String,
    read_only: bool,
    target: String,
) -> impl IntoView {
    let selected = RwSignal::new(parse_name_list(&value.get_untracked()));

    Effect::new(move |_| {
        let stored = parse_name_list(&value.get());
        if selected.get_untracked() != stored {
            selected.set(stored);
        }
    });

    let on_change = Callback::new(move |next: Vec<String>| {
        let encoded = if next.is_empty() {
            String::new()
        } else {
            serde_json::to_string(&next).unwrap_or_default()
        };
        value.set(encoded);
    });

    view! {
        <DocPicker
            id=id
            target=target
            values=selected
            mode=PickerMode::Multiple
            placeholder=placeholder
            read_only=read_only
            on_change=on_change
        />
    }
}

/// The `tags` standard field: the document's tags as chips.
///
/// The wire value stays the comma-separated `Data` string the engine derives
/// `Tag`/`Tag Link` from (`ferris_api::tags`), so the catalogue is unchanged;
/// only the control is now a real multi picker over the `Tag` documents, which
/// also accepts a brand-new tag typed by the user.
#[component]
pub fn TagsField(id: String, value: RwSignal<String>, read_only: bool) -> impl IntoView {
    let selected = RwSignal::new(split_tags(&value.get_untracked()));

    Effect::new(move |_| {
        let stored = split_tags(&value.get());
        if selected.get_untracked() != stored {
            selected.set(stored);
        }
    });

    let on_change = Callback::new(move |next: Vec<String>| {
        value.set(next.join(", "));
    });

    view! {
        <DocPicker
            id=id
            target="Tag".to_string()
            values=selected
            mode=PickerMode::Multiple
            placeholder=t("components.form.field.tags_placeholder")
            read_only=read_only
            allow_free_text=true
            on_change=on_change
        />
    }
}

fn vec_or_empty(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Vec::new()
    } else {
        vec![trimmed.to_string()]
    }
}

/// `_user_tags`-style list: comma separated, trimmed, empty entries dropped.
fn split_tags(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}
