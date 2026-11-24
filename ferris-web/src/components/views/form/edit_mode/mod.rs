//! Field-edit mode: the normal form plus a properties panel per field.
//!
//! Frappe's *Customize Form* edits a **copy of the form**, so a change needs a
//! separate screen. The Desk edits the real form instead: the layout route
//! (`/desk/:module/:doctype/layout`) overlays every field with a transparent
//! target, and clicking one opens its properties in the sidebar. The change is
//! a **Property Setter** (or the removal of one), applied to the metadata the
//! very form is rendering — the field keeps its place, its section and its tab.
//!
//! The form *is* the field list. There is no in-panel list, no "back to all
//! fields" and no exit inside the panel: clicking a field selects it, the X
//! clears the selection, the `+` rules create fields, and the mode is left from
//! the top bar's "Salir del modo edición". Reordering is drag and drop on the
//! field surfaces (keyboard users edit `insert_after` in the panel).
//!
//! Only the properties the server actually applies are offered
//! (`ferris-schema/src/customization`: `fieldtype`, label, options, default,
//! description, `depends_on`, placeholder, `reqd`, hidden, `read_only`,
//! `in_list_view`, `in_standard_filter`, `unique`, plus the positional
//! `insert_after`). The panel shows the field's **effective** values, marks the
//! ones that come from a setter, and treats clearing a value as "restore the
//! declared one".
//!
//! Mobile: the properties panel is the right column from `lg` up; below that it
//! is a bottom sheet over the form, with a backdrop, so tapping a field never
//! leaves the user scrolling to the bottom of a long form.

mod create;
mod export;
mod field;
mod field_props;
mod props;
mod schema;
mod settings;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use crate::api::types::Customization;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::icon::Icon;
use crate::components::ui::select_native::SelectNative;
use crate::i18n::{t, use_i18n};
use crate::views::services::FormService;

pub(crate) use create::CreateFieldDialog;
use field::FieldInspector;
pub(crate) use settings::DocSettingsDialog;

/// What a field overlay, the insert rules and the inspector share.
#[derive(Clone, Copy)]
pub struct FieldEditCtx {
    pub editing: Signal<bool>,
    pub selected: RwSignal<Option<String>>,
    /// The "new field" dialog: `creating` opens it, and `creating_after` is the
    /// `insert_after` anchor of the rule that was clicked (`__first__` = before
    /// the first field, empty = at the end). The insert rules live in
    /// `layout.rs`; the dialog is mounted by the form itself, outside the right
    /// column, because a field must be creatable while that column is closed.
    pub creating: RwSignal<bool>,
    pub creating_after: RwSignal<String>,
    /// Bumped after a field is created. The inspector caches the doctype's
    /// customization to mark overrides and to offer "Eliminar"; without this
    /// the field it just created would not be known as a custom field until
    /// the next save.
    pub custom_revision: RwSignal<u64>,
    /// The field being dragged on the surface, and the one it is hovering.
    /// Both are names, not indices, because the metadata is reloaded after
    /// every drop and an index would then point at another field.
    pub dragging: RwSignal<Option<String>>,
    pub drop_target: RwSignal<Option<String>>,
    /// Move the dragged field before the target. Resolved by the form service,
    /// which owns the metadata write and the undo entry.
    pub reorder: Callback<(String, String)>,
}

/// The properties panel of the form's edit mode: exactly one field.
#[component]
pub fn InspectorPanel(service: FormService) -> impl IntoView {
    let i18n = use_i18n();
    let Some(edit) = use_context::<FieldEditCtx>() else {
        // The panel only exists inside the form's edit context: without it there
        // is no selection to follow and no dialog to open.
        return view! { <span></span> }.into_any();
    };

    let doctype = service.doctype;
    let meta = service.meta;
    let selected = edit.selected;
    let editing = service.editing;

    let custom = RwSignal::new(None::<Customization>);
    // The header owns the export button (next to the title) and the inspector
    // owns the dialog, so the trigger travels as a signal.
    let export_open = RwSignal::new(false);
    // The picker mirrors the selection; `SelectNative` takes a `ReadSignal`, so
    // the mirror is a signal of its own instead of a derived one.
    let picker = RwSignal::new(selected.get_untracked().unwrap_or_default());
    Effect::new(move |_| picker.set(selected.get().unwrap_or_default()));
    // `Copy` handle for the properties panel, whose `Show` children are rebuilt.
    let service_for_field = StoredValue::new(service);

    // The setters currently in force for this doctype: the panel marks the
    // properties they override, and "Restablecer" removes exactly those.
    let load_custom = move || {
        let dt = doctype.get_untracked();
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                return;
            };
            if let Ok(found) = store.get_customization(&dt).await {
                custom.set(Some(found));
            }
        });
    };
    // Entering edit mode primes the cache. It is re-read when a field is
    // created (the revision the form bumps), because a brand-new custom field is
    // not in the previous snapshot and would otherwise be missing its "campo
    // personalizado" marker and its delete button.
    let revision = edit.custom_revision;
    Effect::new(move |_| {
        if editing.get() {
            let _ = revision.get();
            load_custom();
        }
    });

    view! {
        <Show when=move || selected.get().is_some()>
            <button
                type="button"
                class="fixed inset-0 z-30 bg-overlay/40 lg:hidden"
                aria-label=t("components.form.edit_mode.close_properties")
                on:click=move |_| selected.set(None)
            ></button>
        </Show>

        // In edit mode the column is always the property panel: without a
        // selector there was no way back to a hidden field (it has no overlay)
        // or to a section/tab break (the layout consumes it), so its properties
        // could never be edited again after the first save.
        <Show when=move || editing.get()>
            <aside
                class="flex w-full min-w-0 flex-col gap-3 rounded-xl border bg-card text-sm shadow-xs max-lg:fixed max-lg:inset-x-0 max-lg:bottom-0 max-lg:z-40 max-lg:max-h-[85dvh] max-lg:overflow-y-auto max-lg:rounded-t-2xl max-lg:border-t max-lg:shadow-2xl"
                aria-label=t("components.form.edit_mode.field_properties")
                data-name="FieldInspectorPanel"
            >
                // The header stays at the top of the panel while the properties
                // scroll under it: with a long field form the export/close
                // controls must never be the thing that scrolled away.
                <div class="sticky top-0 z-10 flex items-center justify-between gap-2 rounded-t-xl border-b bg-card px-4 pt-3 pb-2">
                    <div class="flex min-w-0 items-center gap-1">
                        <h3 class="truncate text-sm font-semibold">{t("components.form.edit_mode.properties")}</h3>
                        <Button
                            variant=ButtonVariant::Ghost
                            size=ButtonSize::IconSm
                            attr:aria-label=t("components.form.export_field.title")
                            attr:title=t("components.form.export_field.tooltip")
                            on:click=move |_| export_open.set(true)
                        >
                            <Icon name="download" class="size-4" />
                        </Button>
                    </div>
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::IconSm
                        attr:aria-label=t("components.form.edit_mode.close_properties")
                        attr:title=t("components.form.edit_mode.close_properties")
                        on:click=move |_| selected.set(None)
                    >
                        <Icon name="x" class="size-4" />
                    </Button>
                </div>

                <div class="flex min-w-0 flex-col gap-3 px-4 pb-4">
                    <label class="flex flex-col gap-1" for="field-picker">
                        <span class="text-xs font-semibold tracking-wide text-muted-foreground uppercase">
                            {i18n.t("components.form.edit_mode.pick_field")}
                        </span>
                        <SelectNative
                            id="field-picker"
                            value=picker.read_only()
                            aria_label=i18n.t("components.form.edit_mode.pick_field")
                            on_change=Callback::new(move |ev: leptos::ev::Event| {
                                let value = event_target_value(&ev);
                                picker.set(value.clone());
                                selected.set((!value.is_empty()).then_some(value));
                            })
                        >
                            <option value="">{i18n.t("components.form.edit_mode.pick_field_empty")}</option>
                            {move || {
                                meta.get()
                                    .unwrap_or_default()
                                    .fields
                                    .into_iter()
                                    .map(|field| {
                                        let label = field.label.clone().unwrap_or_else(|| field.fieldname.clone());
                                        let text = format!("{label} · {}", field.fieldtype);
                                        view! { <option value=field.fieldname>{text}</option> }
                                    })
                                    .collect_view()
                            }}
                        </SelectNative>
                    </label>
                    {move || match selected.get() {
                        None => ().into_any(),
                        Some(name) => {
                            // The inspector's markers ("editado", the delete
                            // button) refresh when a customization lands; reading
                            // `custom` here is what re-renders them after a create
                            // or a reset.
                            let _ = custom.get();
                            // Tracked on purpose: a brand-new custom field is not
                            // in the metadata snapshot the write was based on, so
                            // the selection set by the create dialog resolves to
                            // "not found" until the reload lands. Reading
                            // untracked skipped that update and left the inspector
                            // on the placeholder forever.
                            match meta.get().and_then(|meta| {
                                meta.fields.into_iter().find(|field| field.fieldname == name)
                            }) {
                                Some(field) => view! {
                                    <FieldInspector
                                        field=field
                                        service=service_for_field.get_value()
                                        custom=custom
                                        export_open=export_open
                                        on_saved=Callback::new(move |()| load_custom())
                                    />
                                }.into_any(),
                                None => view! {
                                    <p class="text-xs text-destructive">{i18n.t("components.form.edit_mode.field_not_found")}</p>
                                }.into_any(),
                            }
                        }
                    }}
                </div>
            </aside>
        </Show>
    }
    .into_any()
}
