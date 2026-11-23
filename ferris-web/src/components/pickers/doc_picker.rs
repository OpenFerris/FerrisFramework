//! The universal document picker.
//!
//! One control serves every place the Desk references another document: a
//! `Link` field, a `Table MultiSelect` (tags, projects), the share dialog, the
//! dashboard parameter panel, … It replaces the per-widget copies that had
//! drifted apart (no debounce in one, no keyboard in another, no multi-select in
//! a third).
//!
//! Behaviour:
//! - a text box opens a listbox with **at most `limit` matches** (five by
//!   default), searched through `POST /api/resource/{doctype}/list` with the
//!   server-side `search` parameter;
//! - typing is debounced (200 ms) and every answer carries a sequence number, so
//!   a slow response for an older query can never overwrite the current matches;
//! - `Up`/`Down` move the active option, `Enter` picks it and `Escape` closes;
//!   `Backspace` on an empty query removes the last chip in multi mode;
//! - multi mode keeps the picks as chips and is what tags and multi-select
//!   fields use;
//! - the displayed label is the document's human name (`full_name` for users,
//!   `subject`/`title`/… for the rest) while the stored value stays the document
//!   name, so the wire contract does not change.
//!
//! The delayed work re-checks its `StoredValue` with the fallible accessors: the
//! picker can be unmounted (tab switch, navigation) while a request is in
//! flight, and `with_value` would panic then.

use std::collections::HashMap;

use icons::{Search, X};
use leptos::ev::KeyboardEvent;
use leptos::prelude::*;

use crate::api::DocStore;
use crate::api::types::ListParams;
use crate::api::{is_opaque_name, link_display};
use crate::components::hooks::use_click_outside::use_click_outside;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::input::{Input, InputType};
use crate::components::ui::spinner::Spinner;
use crate::i18n::{t, use_i18n};

/// One document offered by the listbox.
#[derive(Clone, PartialEq, Eq)]
struct Hit {
    /// Wire value: the document name.
    name: String,
    /// Human label (what the row shows).
    label: String,
    /// Whether the raw name is worth showing next to the label: only when the
    /// label comes from a declared `display_field` (the heuristic already
    /// appends the name), and never for an opaque id.
    suffix: bool,
}

/// How many values the picker holds.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum PickerMode {
    /// `Link`: exactly zero or one name.
    #[default]
    Single,
    /// `Table MultiSelect` / tags / shares: any number of names.
    Multiple,
}

/// Universal picker over the documents of one DocType.
///
/// `values` is the selection (document names, never labels). The caller keeps
/// owning it: the picker writes and also reports each change through
/// `on_change`, which is how a form field serializes to its wire shape.
#[component]
pub fn DocPicker(
    /// Id of the text input (the form field's `id`).
    id: String,
    /// DocType to search. Empty disables the search (a `Dynamic Link` whose
    /// target lives in another field stays usable as typed text).
    target: String,
    /// Selected document names.
    values: RwSignal<Vec<String>>,
    /// Single (`Link`) or multiple (`Table MultiSelect`).
    #[prop(default = PickerMode::Single)]
    mode: PickerMode,
    /// Field to show as the row label instead of the generic heuristic.
    #[prop(optional, into)]
    display_field: Option<String>,
    #[prop(optional, into)] placeholder: Option<String>,
    #[prop(optional)] read_only: bool,
    /// Maximum rows in the listbox.
    #[prop(default = 5usize)]
    limit: usize,
    /// Render the selection as chips inside the control. The sidebar keeps its
    /// chips in its own section and uses the picker as a pure search box, so it
    /// turns this off to avoid showing the same tags twice.
    #[prop(default = true)]
    show_chips: bool,
    /// Focus the search box when the control mounts (the sidebar opens a picker
    /// with its `+` button, and the user expects to type immediately).
    #[prop(optional)]
    autofocus: bool,
    /// Accept a value the DocType has no document for (tags, free-form
    /// references): `Enter` on a query with no matches adds the typed text.
    #[prop(optional)]
    allow_free_text: bool,
    /// Called with the new selection after every change, including typing in
    /// single mode (a `Link` may be written free-hand).
    #[prop(optional, into)]
    on_change: Option<Callback<Vec<String>>>,
    /// Called only when the user picks an option or removes a chip, never for
    /// free typing. The share/assignment pickers use it: the text being typed is
    /// not a selected user yet.
    #[prop(optional, into)]
    on_select: Option<Callback<Vec<String>>>,
) -> impl IntoView {
    let i18n = use_i18n();
    let chip_i18n = i18n;
    let empty_i18n = i18n;
    let has_target = !target.is_empty();
    let multiple = mode == PickerMode::Multiple;
    let input_id = id.clone();
    let listbox_id = format!("{id}-listbox");
    let listbox_for_input = listbox_id.clone();
    let listbox_for_list = listbox_id.clone();
    let container_ref = NodeRef::<leptos::html::Div>::new();
    let input_ref = NodeRef::<leptos::html::Input>::new();
    let display_field = StoredValue::new(display_field);
    let target_store = StoredValue::new(target.clone());
    let limit = limit.max(1);

    // Single mode: the input text *is* the wire value (a Link may also be typed
    // free-hand, e.g. a Dynamic Link). Multi mode: the input is only the query.
    let query = RwSignal::new(
        values
            .get_untracked()
            .first()
            .cloned()
            .filter(|_| !multiple)
            .unwrap_or_default(),
    );
    let rows = RwSignal::new(Vec::<Hit>::new());
    let labels = RwSignal::new(HashMap::<String, String>::new());
    let loading = RwSignal::new(false);
    let open = RwSignal::new(false);
    let active = RwSignal::new(0usize);
    // Guards out-of-order answers and tells a disposed picker apart.
    let seq = StoredValue::new(0u64);

    // External changes (document load, another widget) flow into the input.
    Effect::new(move |_| {
        if multiple {
            return;
        }
        let current = values.get().first().cloned().unwrap_or_default();
        if query.get_untracked() != current {
            query.set(current);
        }
    });

    // Resolve the labels of the selected names that no search has seen yet.
    Effect::new(move |_| {
        // Only chips need a label, and a value the user just typed is not a
        // document: fetching it produced a 404 per keystroke. Picks carry their
        // label already (and free text is labelled with itself).
        if !multiple {
            return;
        }
        let selected = values.get();
        let known = labels.get();
        for name in selected {
            if name.trim().is_empty() || known.contains_key(&name) {
                continue;
            }
            let target = target_store.get_value();
            if target.is_empty() {
                continue;
            }
            let field = display_field.get_value();
            crate::api::drive(async move {
                let Ok(store) = DocStore::runtime() else {
                    return;
                };
                if let Ok(doc) = store.get(&target, &name).await {
                    let label = display(field.as_deref(), &doc);
                    labels.update(|map| {
                        map.insert(name, label);
                    });
                }
            });
        }
    });

    // Debounced search. An empty query lists the first page of the DocType, so
    // opening the control already offers choices.
    Effect::new(move |_| {
        let q = query.get().trim().to_string();
        if !has_target || !open.get() {
            return;
        }
        let generation = seq.try_with_value(|s| *s).unwrap_or(0) + 1;
        seq.set_value(generation);
        loading.set(true);
        let target = target_store.get_value();
        let field = display_field.get_value();
        set_timeout(
            move || {
                crate::api::drive(async move {
                    if seq.try_with_value(|s| *s) != Some(generation) {
                        return;
                    }
                    let Ok(store) = DocStore::runtime() else {
                        if seq.try_with_value(|s| *s) == Some(generation) {
                            loading.set(false);
                        }
                        return;
                    };
                    let params = ListParams {
                        search: (!q.is_empty()).then_some(q.clone()),
                        limit_page_length: limit as u64,
                        ..Default::default()
                    };
                    let answer = store.list(&target, &params).await;
                    if seq.try_with_value(|s| *s) != Some(generation) {
                        return;
                    }
                    match answer {
                        Ok(response) => {
                            let hits: Vec<Hit> = response
                                .data
                                .iter()
                                .map(|doc| {
                                    let name = doc
                                        .get("name")
                                        .and_then(serde_json::Value::as_str)
                                        .unwrap_or_default()
                                        .to_string();
                                    let label = display(field.as_deref(), doc);
                                    Hit {
                                        suffix: field.is_some()
                                            && !is_opaque_name(&name)
                                            && label != name,
                                        name,
                                        label,
                                    }
                                })
                                .filter(|hit| !hit.name.is_empty())
                                .collect();
                            labels.update(|map| {
                                for hit in &hits {
                                    map.insert(hit.name.clone(), hit.label.clone());
                                }
                            });
                            active.set(0);
                            rows.set(hits);
                        }
                        Err(_) => rows.set(Vec::new()),
                    }
                    loading.set(false);
                });
            },
            std::time::Duration::from_millis(200),
        );
    });

    use_click_outside(container_ref, move || open.set(false));

    // `+` in the sidebar mounts the picker already focused. Runs once: a later
    // re-render must not steal the cursor from wherever the user moved it.
    let focused = StoredValue::new(false);
    Effect::new(move |_| {
        if !autofocus || focused.get_value() {
            return;
        }
        focused.set_value(true);
        if let Some(input) = input_ref.get_untracked() {
            let _ = input.focus();
        }
    });

    let report = move |next: Vec<String>| {
        if let Some(callback) = on_change {
            callback.run(next);
        }
    };
    let select = move |next: Vec<String>| {
        if let Some(callback) = on_select {
            callback.run(next.clone());
        }
        if let Some(callback) = on_change {
            callback.run(next);
        }
    };

    // A pick writes the selection and reports it; the query is cleared in multi
    // mode so the next search starts blank.
    let pick = Callback::new(move |hit: Hit| {
        let next = if multiple {
            let mut current = values.get_untracked();
            if !current.iter().any(|name| name == &hit.name) {
                current.push(hit.name.clone());
            }
            current
        } else {
            vec![hit.name.clone()]
        };
        values.set(next.clone());
        labels.update(|map| {
            map.insert(hit.name.clone(), hit.label.clone());
        });
        if multiple {
            query.set(String::new());
            if let Some(input) = input_ref.get_untracked() {
                let _ = input.focus();
            }
        } else {
            query.set(hit.name.clone());
            open.set(false);
        }
        select(next);
    });

    let remove = Callback::new(move |name: String| {
        let next: Vec<String> = values
            .get_untracked()
            .into_iter()
            .filter(|value| value != &name)
            .collect();
        values.set(next.clone());
        if !multiple {
            query.set(String::new());
        }
        select(next);
    });

    let on_input = move |event: leptos::ev::Event| {
        let text = event_target_value(&event);
        query.set(text.clone());
        open.set(true);
        if !multiple {
            let next = if text.trim().is_empty() {
                Vec::new()
            } else {
                vec![text]
            };
            values.set(next.clone());
            report(next);
        }
    };

    let on_keydown = move |event: KeyboardEvent| {
        let list = rows.get_untracked();
        match event.key().as_str() {
            "ArrowDown" if !list.is_empty() => {
                event.prevent_default();
                active.update(|index| *index = (*index + 1).min(list.len() - 1));
            }
            "ArrowUp" if !list.is_empty() => {
                event.prevent_default();
                active.update(|index| *index = index.saturating_sub(1));
            }
            "Enter" if open.get_untracked() && !list.is_empty() => {
                event.prevent_default();
                let index = active.get_untracked().min(list.len() - 1);
                pick.run(list[index].clone());
            }
            "Enter" if allow_free_text && !query.get_untracked().trim().is_empty() => {
                event.prevent_default();
                let text = query.get_untracked().trim().to_string();
                pick.run(Hit {
                    name: text.clone(),
                    label: text,
                    suffix: false,
                });
            }
            "Escape" => {
                event.prevent_default();
                open.set(false);
            }
            "Backspace" if multiple && query.get_untracked().is_empty() => {
                if let Some(last) = values.get_untracked().last().cloned() {
                    remove.run(last);
                }
            }
            _ => {}
        }
    };

    let label_for = move |name: &str| {
        labels
            .get()
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string())
    };

    let id_for_active = id.clone();
    // `StoredValue` is `Copy`: the list closure lives inside a `Show` whose
    // children must stay `Fn`, and moving the `String` into it would make them
    // `FnOnce`.
    let id_store = StoredValue::new(id.clone());
    // An empty placeholder from the metadata means "none declared", so the
    // picker generates the search hint instead of rendering an unlabelled box.
    let placeholder = placeholder
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| {
            if has_target {
                i18n.t_with("components.doc_picker.search_in", &[("doctype", &target)])
            } else {
                i18n.t("components.doc_picker.type")
            }
        });

    view! {
        <div class="relative" node_ref=container_ref>
            <div class="flex flex-wrap items-center gap-1">
                {move || {
                    if !multiple || !show_chips {
                        return ().into_any();
                    }
                    values
                        .get()
                        .into_iter()
                        .map(|name| {
                            let title = name.clone();
                            let shown = label_for(&name);
                            view! {
                                <span
                                    class="flex max-w-full min-w-0 items-center gap-1 rounded-md border bg-secondary px-2 py-0.5 text-sm text-secondary-foreground"
                                    title=title
                                >
                                    <span class="min-w-0 truncate">{shown}</span>
                                    <Show when=move || !read_only>
                                        <button
                                            type="button"
                                            class="shrink-0 rounded-sm text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring max-md:-me-1.5 max-md:flex max-md:size-7 max-md:items-center max-md:justify-center"
                                            aria-label=chip_i18n.t_with(
                                                "components.doc_picker.remove",
                                                &[("name", &name)],
                                            )
                                            on:click={
                                                let name = name.clone();
                                                move |_| remove.run(name.clone())
                                            }
                                        >
                                            <X class="size-3" />
                                        </button>
                                    </Show>
                                </span>
                            }
                        })
                        .collect_view()
                        .into_any()
                }}
                <div class="relative min-w-0 flex-1">
                    <Input
                        id=input_id
                        r#type=InputType::Text
                        attr:role="combobox"
                        attr:aria-autocomplete="list"
                        attr:aria-expanded=move || open.get().to_string()
                        attr:aria-controls=listbox_for_input
                        attr:aria-activedescendant=move || {
                            open
                                .get()
                                .then(|| format!("{}-opt-{}", id_for_active, active.get()))
                        }
                        autocomplete="off"
                        placeholder=placeholder
                        readonly=read_only
                        node_ref=input_ref
                        prop:value=move || query.get()
                        on:input=on_input
                        on:focus=move |_| {
                            if !read_only && has_target {
                                open.set(true);
                            }
                        }
                        on:keydown=on_keydown
                    />
                    <div class="pointer-events-none absolute inset-y-0 end-2 flex items-center gap-1">
                        <Show when=move || loading.get() && open.get()>
                            <Spinner class="size-4 text-muted-foreground" />
                        </Show>
                        <Show when=move || !read_only && !values.get().is_empty()>
                            <Button
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::IconXs
                                class="pointer-events-auto text-muted-foreground"
                                attr:aria-label=t("components.doc_picker.clear")
                                on:click=move |_| {
                                    values.set(Vec::new());
                                    query.set(String::new());
                                    select(Vec::new());
                                }
                            >
                                <X class="size-3.5" />
                            </Button>
                        </Show>
                        <Show when=move || has_target && !loading.get() && values.get().is_empty()>
                            <Search class="size-4 text-muted-foreground" />
                        </Show>
                    </div>
                </div>
            </div>

            <Show when=move || open.get()>
                <div
                    id=listbox_for_list.clone()
                    role="listbox"
                    class="absolute z-30 mt-1 max-h-56 w-full overflow-auto rounded-md border bg-popover py-1 text-popover-foreground shadow-lg"
                >
                    <Show when=move || rows.get().is_empty() && !loading.get()>
                        <p class="px-3 py-2 text-sm text-muted-foreground">
                            {if allow_free_text {
                                empty_i18n.t("components.doc_picker.empty_free_text")
                            } else {
                                empty_i18n.t("components.doc_picker.empty")
                            }}
                        </p>
                    </Show>
                    {move || {
                        rows.get()
                            .into_iter()
                            .enumerate()
                            .map(|(index, hit)| {
                                let option_id =
                                    id_store.with_value(|id| format!("{}-opt-{}", id, index));
                                let selected = Signal::derive(move || active.get() == index);
                                let name = hit.name.clone();
                                let show_name = hit.suffix;
                                let label = hit.label.clone();
                                view! {
                                    <button
                                        type="button"
                                        id=option_id
                                        role="option"
                                        aria-selected=move || selected.get().to_string()
                                        class=move || {
                                            if selected.get() {
                                                "block w-full truncate bg-accent px-3 py-1.5 text-start text-sm text-accent-foreground max-md:min-h-10"
                                            } else {
                                                "block w-full truncate px-3 py-1.5 text-start text-sm hover:bg-accent max-md:min-h-10"
                                            }
                                        }
                                        on:click={
                                            let hit = hit.clone();
                                            move |_| pick.run(hit.clone())
                                        }
                                    >
                                        {label.clone()}
                                        <Show when=move || show_name>
                                            <span class="ms-1 text-xs text-muted-foreground">
                                                {name.clone()}
                                            </span>
                                        </Show>
                                    </button>
                                }
                            })
                            .collect_view()
                    }}
                </div>
            </Show>
        </div>
    }
}

/// The row label for a document: `display_field` when declared, otherwise the
/// shared heuristic (which prefers `full_name`, so a user is shown by name).
fn display(field: Option<&str>, doc: &serde_json::Value) -> String {
    if let Some(field) = field
        && let Some(text) = doc.get(field).and_then(serde_json::Value::as_str)
        && !text.trim().is_empty()
    {
        return text.to_string();
    }
    link_display(doc)
}
