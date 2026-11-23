//! Create a field — or a section, a tab, a column — from the layout surface.
//!
//! This is the in-form half of Frappe's *Customize Form*: a new field is a
//! `Custom Field` row anchored after another field, so it keeps the place the
//! user chose as the form grows. Layout breaks are ordinary choices here; the
//! Desk's layout builder turns them into tabs/sections/columns, and a break's
//! `fieldname` is its identity (for a `Tab Break` it *is* the tab value).
//!
//! The custom-field row carries only the properties the server stores on it.
//! Everything else the dialog offers (`placeholder`, `depends_on`,
//! `in_list_view`, `in_standard_filter`) is written as the property setter the
//! engine applies, right after the field exists, so the create path is the same
//! write the inspector performs.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use crate::api::types::{CustomFieldSpec, PropertySetterSpec};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::dialog::{
    Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle,
};
use crate::components::ui::input::{Input, InputType};
use crate::components::ui::select_native::SelectNative;
use crate::i18n::{t, use_i18n};
use crate::views::services::FormService;
use crate::views::services::form_service::LayoutOp;

use super::super::customize::{FIELD_TYPES, hint};

/// The create-field dialog opened from the hairline rules between fields.
///
/// It is a dialog rather than a panel of the inspector on purpose: the rules
/// must create a field while the right column is collapsed (nothing selected),
/// and the anchor is the position the user clicked, not the selected field.
#[component]
pub(crate) fn CreateFieldDialog(
    service: FormService,
    open: RwSignal<bool>,
    anchor: RwSignal<String>,
    on_created: Callback<String>,
) -> impl IntoView {
    // `StoredValue` is `Copy`: the close handler and the panel are built at
    // different times but share the same service.
    let service = StoredValue::new(service);
    let cancel = Callback::new(move |()| open.set(false));

    view! {
        <Dialog>
            <DialogContent open=open class="max-w-md">
                <DialogHeader>
                    <DialogTitle>{t("components.form.edit_mode.new_field")}</DialogTitle>
                    <DialogDescription>
                        {t("components.form.edit_mode.new_field_description")}
                    </DialogDescription>
                </DialogHeader>
                // Built when the dialog opens, so `anchor` is the rule that was
                // clicked rather than the value at mount time. The content stays
                // mounted while closed (the scroll lock is released by the
                // close, not by the unmount), so only the panel is conditional.
                <Show when=move || open.get()>
                    <CreateFieldPanel
                        service=service.get_value()
                        anchor=anchor.get_untracked()
                        on_created=on_created
                        on_cancel=cancel
                    />
                </Show>
            </DialogContent>
        </Dialog>
    }
}

/// The new-field form. `anchor` is prefilled as "insert after": the field the
/// rule sits under, or empty for the end of the form.
#[component]
pub(super) fn CreateFieldPanel(
    service: FormService,
    #[prop(optional)] anchor: String,
    on_created: Callback<String>,
    on_cancel: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    let name = RwSignal::new(String::new());
    let label = RwSignal::new(String::new());
    let fieldtype = RwSignal::new("Data".to_string());
    let options = RwSignal::new(String::new());
    let after = RwSignal::new(anchor);
    let default_value = RwSignal::new(String::new());
    let placeholder = RwSignal::new(String::new());
    let depends_on = RwSignal::new(String::new());
    let reqd = RwSignal::new(false);
    let hidden = RwSignal::new(false);
    let read_only = RwSignal::new(false);
    let in_list_view = RwSignal::new(false);
    let in_standard_filter = RwSignal::new(false);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let reload_signal = service.reload;
    let hint_i18n = i18n;

    let create = Callback::new(move |()| {
        let is_break = matches!(
            fieldtype.get_untracked().as_str(),
            "Tab Break" | "Section Break" | "Column Break"
        );
        let typed_label = label.get_untracked().trim().to_string();
        let mut fieldname = name.get_untracked().trim().to_string();
        // A break is usually named by its label alone; its fieldname is the
        // identity the builder keys on (a tab's value), so it has to exist.
        if fieldname.is_empty() && is_break {
            fieldname = slugify(&typed_label);
        }
        if fieldname.is_empty() {
            error.set(Some(if is_break {
                i18n.t("components.form.edit_mode.break_label_required")
            } else {
                i18n.t("components.form.edit_mode.name_required")
            }));
            return;
        }
        let label_value = nonempty(&typed_label).or_else(|| {
            // A break with no label still needs something to show in the tab
            // strip or the section heading.
            is_break.then(|| fieldname.clone())
        });
        let spec = CustomFieldSpec {
            fieldname: fieldname.clone(),
            label: label_value,
            fieldtype: fieldtype.get_untracked(),
            options: nonempty(&options.get_untracked()),
            insert_after: nonempty(&after.get_untracked()),
            reqd: reqd.get_untracked(),
            default: nonempty(&default_value.get_untracked()),
            hidden: hidden.get_untracked(),
            read_only: read_only.get_untracked(),
        };
        // Properties the custom-field row does not store travel as setters, the
        // same ones the inspector writes. A false flag is simply absent: the
        // declared value already is `false`.
        let extras: Vec<(&'static str, String)> = [
            ("placeholder", placeholder.get_untracked()),
            ("depends_on", depends_on.get_untracked()),
            (
                "in_list_view",
                if in_list_view.get_untracked() {
                    "1".to_string()
                } else {
                    String::new()
                },
            ),
            (
                "in_standard_filter",
                if in_standard_filter.get_untracked() {
                    "1".to_string()
                } else {
                    String::new()
                },
            ),
        ]
        .into_iter()
        .filter_map(|(property, value)| (!value.trim().is_empty()).then_some((property, value)))
        .collect();

        let dt = service.doctype.get_untracked();
        // The callback stays `Fn`: it may run again after a failure, so the
        // service handle for the async write is cloned per invocation instead of
        // moved out of the closure.
        let record_service = service.clone();
        error.set(None);
        saving.set(true);
        let i18n = i18n;
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                saving.set(false);
                return;
            };
            if let Err(message) = store.put_custom_field(&dt, &spec).await {
                saving.set(false);
                error.set(Some(message));
                return;
            }
            // Only a write the server accepted is undoable. The field is
            // recorded before its setters so undo removes the setters first and
            // the field last.
            record_service.record_layout_op(LayoutOp::FieldCreated { spec: spec.clone() });
            let mut failure = None;
            for (property, value) in extras {
                let write = store
                    .put_property_setter(
                        &dt,
                        &PropertySetterSpec {
                            fieldname: Some(fieldname.clone()),
                            property: property.to_string(),
                            value: value.clone(),
                        },
                    )
                    .await;
                match write {
                    Ok(_) => record_service.record_layout_op(LayoutOp::PropertySet {
                        fieldname: Some(fieldname.clone()),
                        property: property.to_string(),
                        previous: None,
                        value,
                    }),
                    Err(message) => {
                        failure = Some(i18n.t_with(
                            "components.form.edit_mode.extra_property_failed",
                            &[("property", property), ("message", &message)],
                        ));
                        break;
                    }
                }
            }
            saving.set(false);
            // The field exists either way: the form has to show it even when one
            // of its extra properties was rejected.
            reload_signal.update(|n| *n += 1);
            if let Some(message) = failure {
                error.set(Some(message));
            }
            on_created.run(fieldname);
        });
    });

    let current_hint = Signal::derive(move || hint(&fieldtype.get(), &hint_i18n));

    view! {
        <div class="flex flex-col gap-2 rounded-lg border bg-card p-2">
            <p class="text-xs font-semibold">{t("components.form.edit_mode.new_field")}</p>
            <Show when=move || error.get().is_some()>
                <p class="text-xs text-destructive" role="alert">
                    {move || error.get().unwrap_or_default()}
                </p>
            </Show>
            <label class="flex flex-col gap-1 text-xs">
                <span class="font-medium text-muted-foreground">{t("components.form.edit_mode.type")}</span>
                <SelectNative
                    id="new-field-type"
                    value=fieldtype.read_only()
                    aria_label=t("components.form.edit_mode.field_type")
                    on_change=Callback::new(move |event: leptos::ev::Event| {
                        fieldtype.set(event_target_value(&event));
                    })
                >
                    {FIELD_TYPES
                        .iter()
                        .map(|fieldtype| {
                            let value = fieldtype.to_string();
                            let shown = value.clone();
                            view! { <option value=value>{shown}</option> }
                        })
                        .collect_view()}
                </SelectNative>
            </label>
            <p class="text-xs text-muted-foreground">{move || current_hint.get()}</p>

            <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
                <label class="flex flex-col gap-1 text-xs">
                    <span class="font-medium text-muted-foreground">{t("components.form.edit_mode.field_name")}</span>
                    <Input
                        id="new-field-name"
                        r#type=InputType::Text
                        placeholder="nombre_del_campo"
                        prop:value=move || name.get()
                        on_input=Callback::new(move |text: String| name.set(text))
                    />
                </label>
                <label class="flex flex-col gap-1 text-xs">
                    <span class="font-medium text-muted-foreground">{t("components.form.edit_mode.label")}</span>
                    <Input
                        id="new-field-label"
                        r#type=InputType::Text
                        placeholder=t("components.form.edit_mode.label_placeholder")
                        prop:value=move || label.get()
                        on_input=Callback::new(move |text: String| label.set(text))
                    />
                </label>
            </div>

            <label class="flex flex-col gap-1 text-xs">
                <span class="font-medium text-muted-foreground">{t("components.form.edit_mode.options")}</span>
                <Input
                    id="new-field-options"
                    r#type=InputType::Text
                    placeholder=t("components.form.edit_mode.options_placeholder")
                    prop:value=move || options.get()
                    on_input=Callback::new(move |text: String| options.set(text))
                />
            </label>

            <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
                <label class="flex flex-col gap-1 text-xs">
                    <span class="font-medium text-muted-foreground">{t("components.form.edit_mode.default_value")}</span>
                    <Input
                        id="new-field-default"
                        r#type=InputType::Text
                        placeholder=t("components.form.edit_mode.optional")
                        prop:value=move || default_value.get()
                        on_input=Callback::new(move |text: String| default_value.set(text))
                    />
                </label>
                <label class="flex flex-col gap-1 text-xs">
                    <span class="font-medium text-muted-foreground">{t("components.form.edit_mode.placeholder_field")}</span>
                    <Input
                        id="new-field-placeholder"
                        r#type=InputType::Text
                        placeholder=t("components.form.edit_mode.optional")
                        prop:value=move || placeholder.get()
                        on_input=Callback::new(move |text: String| placeholder.set(text))
                    />
                </label>
            </div>

            <label class="flex flex-col gap-1 text-xs">
                <span class="font-medium text-muted-foreground">
                    {t("components.form.edit_mode.visibility_condition")}
                </span>
                <Input
                    id="new-field-depends"
                    r#type=InputType::Text
                    placeholder=t("components.form.edit_mode.depends_example")
                    prop:value=move || depends_on.get()
                    on_input=Callback::new(move |text: String| depends_on.set(text))
                />
            </label>

            <label class="flex flex-col gap-1 text-xs">
                <span class="font-medium text-muted-foreground">{t("components.form.edit_mode.insert_after")}</span>
                <Input
                    id="new-field-after"
                    r#type=InputType::Text
                    placeholder=t("components.form.edit_mode.insert_after_placeholder")
                    prop:value=move || after.get()
                    on_input=Callback::new(move |text: String| after.set(text))
                />
            </label>

            <div class="grid grid-cols-2 gap-x-3 gap-y-1">
                <label class="flex items-center justify-between gap-2 text-xs">
                    <span>{t("components.form.edit_mode.required")}</span>
                    <Checkbox
                        checked=Signal::derive(move || reqd.get())
                        aria_label=t("components.form.edit_mode.required")
                        on_checked_change=Callback::new(move |checked: bool| reqd.set(checked))
                    />
                </label>
                <label class="flex items-center justify-between gap-2 text-xs">
                    <span>{t("components.form.edit_mode.hidden")}</span>
                    <Checkbox
                        checked=Signal::derive(move || hidden.get())
                        aria_label=t("components.form.edit_mode.hidden")
                        on_checked_change=Callback::new(move |checked: bool| hidden.set(checked))
                    />
                </label>
                <label class="flex items-center justify-between gap-2 text-xs">
                    <span>{t("components.form.edit_mode.read_only")}</span>
                    <Checkbox
                        checked=Signal::derive(move || read_only.get())
                        aria_label=t("components.form.edit_mode.read_only")
                        on_checked_change=Callback::new(move |checked: bool| read_only.set(checked))
                    />
                </label>
                <label class="flex items-center justify-between gap-2 text-xs">
                    <span>{t("components.form.edit_mode.in_list_view")}</span>
                    <Checkbox
                        checked=Signal::derive(move || in_list_view.get())
                        aria_label=t("components.form.edit_mode.in_list_view")
                        on_checked_change=Callback::new(move |checked: bool| {
                            in_list_view.set(checked);
                        })
                    />
                </label>
                <label class="flex items-center justify-between gap-2 text-xs">
                    <span>{t("components.form.edit_mode.in_filters")}</span>
                    <Checkbox
                        checked=Signal::derive(move || in_standard_filter.get())
                        aria_label=t("components.form.edit_mode.in_filters")
                        on_checked_change=Callback::new(move |checked: bool| {
                            in_standard_filter.set(checked);
                        })
                    />
                </label>
            </div>

            <div class="flex justify-end gap-2">
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    on:click=move |_| on_cancel.run(())
                >
                    {t("common.cancel")}
                </Button>
                <Button
                    variant=ButtonVariant::Default
                    size=ButtonSize::Sm
                    attr:disabled=move || saving.get()
                    on:click=move |_| create.run(())
                >
                    {t("components.list.empty.create")}
                </Button>
            </div>
        </div>
    }
}

fn nonempty(value: &str) -> Option<String> {
    let trimmed = value.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// A fieldname from a free-text label (`Datos generales` → `datos_generales`).
fn slugify(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for ch in label.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if (ch.is_whitespace() || ch == '-' || ch == '_')
            && !out.is_empty()
            && !out.ends_with('_')
        {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}
