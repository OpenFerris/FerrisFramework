//! A settings input that picks one field of the `DocType`.
//!
//! A free-text field name is a trap: a typo is only discovered by opening the
//! view and finding it empty. The picker offers exactly the fields the property
//! accepts (the caller filters by fieldtype), plus an explicit "none" entry for
//! the properties that may be cleared. The server still validates the value —
//! the picker is the good path, not the enforcement point.

use leptos::prelude::*;

use crate::components::ui::select_native::SelectNative;
use crate::i18n::{t, use_i18n};

/// One selectable field: the stored name and the label the user reads.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct FieldOption {
    pub name: String,
    pub label: String,
}

impl FieldOption {
    pub(super) fn new(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
        }
    }
}

/// A labelled select over `options`.
#[component]
pub(super) fn FieldPicker(
    /// Visible label of the setting.
    #[prop(into)]
    title: String,
    /// One-line explanation under the control.
    #[prop(into, optional)]
    hint: String,
    /// The stored property value (empty means "not declared").
    value: RwSignal<String>,
    /// The fields the property accepts.
    #[prop(into)]
    options: Signal<Vec<FieldOption>>,
    /// Whether the user may clear the setting.
    #[prop(default = true)]
    allow_empty: bool,
) -> impl IntoView {
    let missing_i18n = use_i18n();
    let id = format!(
        "doctype-setting-{}",
        title.to_lowercase().replace([' ', '«', '»'], "-")
    );
    let id_for_label = id.clone();
    let has_hint = !hint.trim().is_empty();
    view! {
        <div class="flex flex-col gap-1">
            <label for=id_for_label class="text-xs font-medium text-muted-foreground">
                {title}
            </label>
            <SelectNative
                id=id
                value=value.read_only()
                on_change=Callback::new(move |ev: leptos::ev::Event| {
                    value.set(event_target_value(&ev));
                })
            >
                {allow_empty.then(|| view! { <option value="">{t("components.form.settings.not_declared")}</option> })}
                {move || {
                    let current = value.get();
                    let mut out: Vec<AnyView> = options
                        .get()
                        .into_iter()
                        .map(|option| {
                            let label = if option.label.is_empty() {
                                option.name.clone()
                            } else {
                                option.label.clone()
                            };
                            view! { <option value=option.name>{label}</option> }.into_any()
                        })
                        .collect();
                    // A stored value the doctype no longer declares stays
                    // visible (and selected) instead of silently disappearing:
                    // the user has to see what is in force to fix it.
                    if !current.is_empty()
                        && !out.is_empty()
                        && !options
                            .get_untracked()
                            .iter()
                            .any(|option| option.name == current)
                    {
                        let label = missing_i18n.t_with(
                            "components.form.settings.field_missing",
                            &[("field", &current)],
                        );
                        out.push(view! { <option value=current>{label}</option> }.into_any());
                    }
                    out.into_iter().collect_view()
                }}
            </SelectNative>
            {has_hint.then(|| view! {
                <p class="text-xs text-muted-foreground">{hint}</p>
            })}
        </div>
    }
}

/// A labelled free-text setting (the ones whose value is not a field name).
#[component]
pub(super) fn PropTextSetting(
    #[prop(into)] title: String,
    #[prop(into, optional)] hint: String,
    value: RwSignal<String>,
) -> impl IntoView {
    let id = format!(
        "doctype-setting-{}",
        title.to_lowercase().replace([' ', '«', '»'], "-")
    );
    let id_for_label = id.clone();
    let has_hint = !hint.trim().is_empty();
    view! {
        <div class="flex flex-col gap-1">
            <label for=id_for_label class="text-xs font-medium text-muted-foreground">
                {title}
            </label>
            <input
                id=id
                type="text"
                class="h-9 w-full rounded-md border border-input bg-background px-3 text-sm text-foreground placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 max-md:h-10"
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev))
            />
            {has_hint.then(|| view! {
                <p class="text-xs text-muted-foreground">{hint}</p>
            })}
        </div>
    }
}
