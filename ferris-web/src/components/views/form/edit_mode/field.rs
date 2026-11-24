//! The inspector body for one selected field.
//!
//! The panel shows exactly one field — the one the user clicked on the layout
//! surface — so there is no in-panel list and no way back to one: the form
//! itself is the list. The header's X clears the selection (that is the exit
//! this panel owns); leaving the mode is the top bar's job.
//!
//! Every property exposed here is one the server applies. The draft is diffed
//! against the field's current effective values, each change becomes a
//! `Property Setter`, and a cleared text field removes the setter instead of
//! writing an empty override.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use crate::api::DynField;
use crate::api::types::{Customization, PropertySetterSpec};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::icon::Icon;
use crate::i18n::{t, use_i18n};
use crate::views::services::FormService;
use crate::views::services::form_service::LayoutOp;

use super::export::ExportDialog;
use super::field_props::{FieldPropertyInputs, Overrides};
use super::schema::{Change, FieldDraft};

/// The properties of one field, with save and reset.
#[component]
pub(super) fn FieldInspector(
    field: DynField,
    service: FormService,
    custom: RwSignal<Option<Customization>>,
    export_open: RwSignal<bool>,
    on_saved: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    let save_i18n = i18n;
    let reset_i18n = i18n;
    let selected = service.selected_field;
    let doctype = service.doctype;

    let fieldname = field.fieldname.clone();
    // `StoredValue` is `Copy`: the name is read by several closures and moving
    // the `String` into the first one would make all the others impossible.
    let name_store = StoredValue::new(fieldname.clone());
    let reload_signal = service.reload;
    let fieldtype = field.fieldtype.clone();
    let title = field
        .label
        .clone()
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| fieldname.clone());
    // The anchor in force, read from the setter: `insert_after` is not part of
    // the field contract, only of the customization.
    let insert_after = custom
        .get_untracked()
        .and_then(|custom| {
            custom
                .properties
                .iter()
                .find(|setter| {
                    setter.fieldname.as_deref() == Some(fieldname.as_str())
                        && setter.property == "insert_after"
                })
                .map(|setter| setter.value.clone())
        })
        .unwrap_or_default();
    let draft = FieldDraft::from_field(&field, insert_after);

    let saving = RwSignal::new(false);
    // Applying a property re-reads the document (the form's shape changes), so
    // an unsaved edit would be lost. Inspecting stays allowed; saving does not.
    let dirty = service.dirty;
    let error = RwSignal::new(None::<String>);
    let notice = RwSignal::new(None::<String>);

    // One signal per control: a keystroke updates its own value and nothing
    // above re-renders (the panel is rebuilt only when the selection or the
    // metadata changes).
    let d_fieldtype = RwSignal::new(draft.fieldtype.clone());
    let d_label = RwSignal::new(draft.label.clone());
    let d_options = RwSignal::new(draft.options.clone());
    let d_default = RwSignal::new(draft.default_value.clone());
    let d_description = RwSignal::new(draft.description.clone());
    let d_depends = RwSignal::new(draft.depends_on.clone());
    let d_placeholder = RwSignal::new(draft.placeholder.clone());
    let d_reqd = RwSignal::new(draft.reqd);
    let d_hidden = RwSignal::new(draft.hidden);
    let d_readonly = RwSignal::new(draft.read_only);
    let d_list = RwSignal::new(draft.in_list_view);
    let d_filter = RwSignal::new(draft.in_standard_filter);
    let d_unique = RwSignal::new(draft.unique);
    let d_insert_after = RwSignal::new(draft.insert_after.clone());
    let d_advanced = RwSignal::new(draft.advanced.clone());

    // `get_untracked`: the markers are recomputed when the panel is rebuilt
    // (the parent re-reads the metadata after a save), so reading the signal in
    // the component body only triggered a "outside a reactive context" warning.
    let overrides = Overrides {
        custom,
        fieldname: StoredValue::new(fieldname.clone()),
    };
    let has_overrides = move || overrides.has_any();

    let is_custom = {
        let name = name_store.get_value();
        custom
            .get_untracked()
            .is_some_and(|custom| custom.fields.iter().any(|item| item.fieldname == name))
    };

    let current = draft;
    // The save closure is `Fn` and may run again after a failure, and `service`
    // is not used after this point: moving it in is the only handle it needs.
    let save_service = service;
    let save = Callback::new(move |()| {
        let mut changes: Vec<Change> = Vec::new();
        let mut text = |property: &'static str, before: String, after: String| {
            if before != after {
                changes.push(Change {
                    property,
                    clear: after.trim().is_empty(),
                    value: after,
                });
            }
        };
        text(
            "fieldtype",
            current.fieldtype.clone(),
            d_fieldtype.get_untracked(),
        );
        text("label", current.label.clone(), d_label.get_untracked());
        text(
            "options",
            current.options.clone(),
            d_options.get_untracked(),
        );
        text(
            "default",
            current.default_value.clone(),
            d_default.get_untracked(),
        );
        text(
            "description",
            current.description.clone(),
            d_description.get_untracked(),
        );
        text(
            "depends_on",
            current.depends_on.clone(),
            d_depends.get_untracked(),
        );
        text(
            "placeholder",
            current.placeholder.clone(),
            d_placeholder.get_untracked(),
        );
        text(
            "insert_after",
            current.insert_after.clone(),
            d_insert_after.get_untracked(),
        );
        let mut flag = |property: &'static str, before: bool, after: bool| {
            if before != after {
                changes.push(Change {
                    property,
                    clear: !after,
                    value: if after { "1".into() } else { "0".into() },
                });
            }
        };
        flag("reqd", current.reqd, d_reqd.get_untracked());
        flag("hidden", current.hidden, d_hidden.get_untracked());
        flag("read_only", current.read_only, d_readonly.get_untracked());
        flag("in_list_view", current.in_list_view, d_list.get_untracked());
        flag(
            "in_standard_filter",
            current.in_standard_filter,
            d_filter.get_untracked(),
        );
        flag("unique", current.unique, d_unique.get_untracked());
        // The advanced table: one diff per property, with the same
        // "empty/false clears the override" rule the rest of the panel uses.
        let advanced = d_advanced.get_untracked();
        for (property, kind) in super::schema::ADVANCED {
            let before = current.advanced.get(*property).cloned().unwrap_or_default();
            let after = advanced.get(*property).cloned().unwrap_or_default();
            if before == after {
                continue;
            }
            let flag_kind = *kind == super::schema::AdvancedKind::Flag;
            changes.push(Change {
                property,
                clear: if flag_kind {
                    after != "1"
                } else {
                    after.trim().is_empty()
                },
                value: after,
            });
        }

        if changes.is_empty() {
            notice.set(Some(save_i18n.t("components.form.edit_mode.no_changes")));
            return;
        }

        let dt = doctype.get_untracked();
        let name = name_store.get_value();
        // What each property replaces, read *before* the write: `previous` is
        // the setter that was in force, and `None` means there was none, so undo
        // must delete rather than restore an empty value.
        let previous: Vec<Option<String>> = {
            let known = custom.get_untracked();
            changes
                .iter()
                .map(|change| {
                    known.as_ref().and_then(|custom| {
                        custom
                            .properties
                            .iter()
                            .find(|setter| {
                                setter.fieldname.as_deref() == Some(name.as_str())
                                    && setter.property == change.property
                            })
                            .map(|setter| setter.value.clone())
                    })
                })
                .collect()
        };
        // The callback is `Fn` and may run again after a failure, so the handle
        // for the async write is cloned instead of moved out of it.
        let record_service = save_service.clone();
        error.set(None);
        notice.set(None);
        saving.set(true);
        let i18n = save_i18n;
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                saving.set(false);
                return;
            };
            let mut failure = None;
            for (change, previous) in changes.into_iter().zip(previous) {
                let result = if change.clear {
                    store
                        .delete_property_setter(&dt, Some(&name), change.property)
                        .await
                } else {
                    store
                        .put_property_setter(
                            &dt,
                            &PropertySetterSpec {
                                fieldname: Some(name.clone()),
                                property: change.property.to_string(),
                                value: change.value.clone(),
                            },
                        )
                        .await
                        .map(|_| ())
                };
                if let Err(message) = result {
                    failure = Some(message);
                    break;
                }
                // Recorded only after the server accepted it, so undo never
                // reverts a change that was rejected.
                record_service.record_layout_op(LayoutOp::PropertySet {
                    fieldname: Some(name.clone()),
                    property: change.property.to_string(),
                    previous,
                    value: change.value,
                });
            }
            saving.set(false);
            if let Some(message) = failure {
                error.set(Some(message));
            } else {
                notice.set(Some(i18n.t("components.form.edit_mode.properties_saved")));
                // The field shape changed: the form re-reads its metadata
                // and rebuilds. The selection lives on the service, so the
                // panel stays on the same field.
                reload_signal.update(|n| *n += 1);
                on_saved.run(());
            }
        });
    });

    let reset = Callback::new(move |()| {
        let name = name_store.get_value();
        let properties: Vec<String> = custom
            .get_untracked()
            .map(|custom| {
                custom
                    .properties
                    .iter()
                    .filter(|setter| setter.fieldname.as_deref() == Some(name.as_str()))
                    .map(|setter| setter.property.clone())
                    .collect()
            })
            .unwrap_or_default();
        if properties.is_empty() {
            return;
        }
        let dt = doctype.get_untracked();
        error.set(None);
        saving.set(true);
        let i18n = reset_i18n;
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                saving.set(false);
                return;
            };
            for property in properties {
                if let Err(message) = store
                    .delete_property_setter(&dt, Some(&name), &property)
                    .await
                {
                    error.set(Some(message));
                    saving.set(false);
                    return;
                }
            }
            saving.set(false);
            notice.set(Some(i18n.t("components.form.edit_mode.field_reset")));
            reload_signal.update(|n| *n += 1);
            on_saved.run(());
        });
    });

    let delete_field = Callback::new(move |()| {
        let name = name_store.get_value();
        let dt = doctype.get_untracked();
        let reload = reload_signal;
        error.set(None);
        saving.set(true);
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                saving.set(false);
                return;
            };
            match store.delete_custom_field(&dt, &name).await {
                Ok(()) => {
                    saving.set(false);
                    // The field no longer exists: clear the selection, which is
                    // also how the panel collapses.
                    selected.set(None);
                    reload.update(|n| *n += 1);
                    on_saved.run(());
                }
                Err(message) => {
                    saving.set(false);
                    error.set(Some(message));
                }
            }
        });
    });

    let is_break = matches!(
        fieldtype.as_str(),
        "Tab Break" | "Section Break" | "Column Break"
    );

    view! {
        <div class="flex min-w-0 flex-col gap-0.5">
            <div class="flex items-start justify-between gap-2">
                <p class="min-w-0 truncate text-sm font-semibold">{title}</p>
                <span
                    class="mt-0.5 inline-flex shrink-0 items-center text-muted-foreground"
                    title=t("components.form.edit_mode.drag_hint")
                    aria-hidden="true"
                >
                    <Icon name="grip-vertical" class="size-4" />
                </span>
            </div>
            <div class="flex flex-wrap items-center gap-1">
                <span class="rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
                    {fieldtype}
                </span>
                <span class="truncate text-xs text-muted-foreground">{fieldname.clone()}</span>
                <Show when=move || is_custom>
                    <span class="rounded bg-secondary px-1.5 py-0.5 text-xs text-secondary-foreground">
                        {t("components.form.edit_mode.custom_field")}
                    </span>
                </Show>
                <Show when=move || is_break>
                    <span class="rounded bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
                        {t("components.form.edit_mode.layout_break")}
                    </span>
                </Show>
            </div>
            <div class="mt-1 flex items-center gap-1">
                <Show when=move || is_custom>
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::IconSm
                        class="text-destructive"
                        attr:aria-label=t("components.form.edit_mode.delete_field")
                        attr:title=t("components.form.edit_mode.delete_custom_field")
                        attr:disabled=move || saving.get()
                        on:click=move |_| delete_field.run(())
                    >
                        <Icon name="trash-2" class="size-4" />
                    </Button>
                </Show>
            </div>
        </div>

        <Show when=move || error.get().is_some()>
            <p
                class="rounded-md border border-destructive/40 bg-destructive/10 px-2 py-1.5 text-xs text-destructive"
                role="alert"
            >
                {move || error.get().unwrap_or_default()}
            </p>
        </Show>
        <Show when=move || notice.get().is_some()>
            <p class="rounded-md border bg-muted px-2 py-1.5 text-xs text-muted-foreground">
                {move || notice.get().unwrap_or_default()}
            </p>
        </Show>

        <FieldPropertyInputs
            fieldtype=d_fieldtype
            label=d_label
            description=d_description
            options=d_options
            default_value=d_default
            placeholder=d_placeholder
            depends_on=d_depends
            reqd=d_reqd
            read_only=d_readonly
            hidden=d_hidden
            unique=d_unique
            in_list_view=d_list
            in_standard_filter=d_filter
            insert_after=d_insert_after
            advanced=d_advanced
            overrides=overrides
            is_break=is_break
        />

        <div class="flex flex-col gap-2 border-t pt-3">
            <Show when=move || dirty.get()>
                <p class="rounded-md border bg-muted px-2 py-1.5 text-xs text-muted-foreground">
                    {t("components.form.edit_mode.save_document_first")}
                </p>
            </Show>
            <div class="flex flex-wrap items-center gap-2">
                <Button
                    variant=ButtonVariant::Default
                    size=ButtonSize::Sm
                    attr:disabled=move || saving.get() || dirty.get()
                    on:click=move |_| save.run(())
                >
                    {t("components.form.edit_mode.save_properties")}
                </Button>
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    attr:disabled=move || saving.get() || dirty.get() || !has_overrides()
                    on:click=move |_| reset.run(())
                >
                    {t("components.form.edit_mode.reset")}
                </Button>
            </div>
        </div>

        <ExportDialog
            open=export_open
            doctype=doctype
            fieldname=fieldname
        />
    }
}
