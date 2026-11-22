//! "Exportar fixtures": write this DocType's customizations into a plugin
//! world.
//!
//! A runtime customization lives in the database (`tab_custom_field` /
//! `tab_property_setter`), which means it does not travel with the app that
//! needed it: a fresh deployment starts without it. The export endpoint copies
//! it into `plugins/<app>/customizations/<doctype>.json`, and `ferris migrate`
//! applies those files back on boot — Frappe's fixtures model.
//!
//! Two scopes, because both are real workflows: export *everything* customized
//! on this DocType (the default, and what makes a world reproducible), or only
//! the changes the user ticked (shipping one field without dragging along a
//! colleague's layout experiment).
//!
//! The scope is a single [`ExportScope`] value, so the two kinds of change are
//! mutually exclusive in one export. When the user ticks both kinds the dialog
//! says so instead of silently dropping half the selection: an export that
//! quietly ignored a box the user checked would be a lie about the world file.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use crate::api::types::{Customization, ExportScope, PropertySetterSpec};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::dialog::{
    Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
};
use crate::components::ui::icon::Icon;
use crate::components::ui::input::{Input, InputType};
use crate::components::ui::radio_button::{RadioGroup, RadioGroupItem};
use crate::components::ui::separator::Separator;

use super::toast::Toasts;

/// The scope value that exports every customization of the DocType.
const SCOPE_ALL: &str = "all";
/// The scope value that exports the ticked changes only.
const SCOPE_SOME: &str = "some";

/// The export dialog opened from the form's "⋯" menu.
#[component]
pub(super) fn ExportCustomizationDialog(
    open: RwSignal<bool>,
    doctype: Signal<String>,
) -> impl IntoView {
    let toasts = Toasts::from_context();

    let loading = RwSignal::new(false);
    let exporting = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    // The path the server wrote, shown as the confirmation.
    let written = RwSignal::new(None::<String>);
    let app = RwSignal::new(String::new());
    let scope = RwSignal::new(SCOPE_ALL.to_string());
    let customization = RwSignal::new(None::<Customization>);
    let picked_fields = RwSignal::new(Vec::<String>::new());
    // `Properties` is keyed by `(fieldname, property)`, and a DocType-level
    // setter has no fieldname at all (`None`), which is not the same as the
    // empty string.
    let picked_properties = RwSignal::new(Vec::<(Option<String>, String)>::new());

    // Reload the current customization every time the dialog opens: a field
    // added since the last export has to be offered, and a stale list would
    // export a world that no longer matches the database.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        error.set(None);
        written.set(None);
        picked_fields.set(Vec::new());
        picked_properties.set(Vec::new());
        let dt = doctype.get_untracked();
        if dt.is_empty() {
            return;
        }
        loading.set(true);
        spawn_local(async move {
            let result = match DocStore::runtime() {
                Ok(store) => store.get_customization(&dt).await,
                Err(message) => Err(message),
            };
            loading.set(false);
            match result {
                Ok(current) => customization.set(Some(current)),
                Err(message) => error.set(Some(message)),
            }
        });
    });

    let submit = move || {
        let target_app = app.get_untracked().trim().to_string();
        if target_app.is_empty() {
            error.set(Some("Escribe el nombre de la app.".to_string()));
            return;
        }
        let scope_value = if scope.get_untracked() == SCOPE_ALL {
            None
        } else {
            let fields = picked_fields.get_untracked();
            let properties = picked_properties.get_untracked();
            match (fields.is_empty(), properties.is_empty()) {
                (true, true) => {
                    error.set(Some("Elige al menos un cambio.".to_string()));
                    return;
                }
                (false, false) => {
                    error.set(Some(
                        "Un tipo de cambio por exportación: elige solo campos o solo propiedades."
                            .to_string(),
                    ));
                    return;
                }
                (false, true) => Some(ExportScope::Fields(fields)),
                (true, false) => Some(ExportScope::Properties(properties)),
            }
        };
        let dt = doctype.get_untracked();
        error.set(None);
        written.set(None);
        exporting.set(true);
        let toasts = toasts.clone();
        spawn_local(async move {
            let result = export(&dt, &target_app, scope_value).await;
            exporting.set(false);
            match result {
                Ok(path) => {
                    toasts.success(format!("Customización exportada a {path}"));
                    written.set(Some(path));
                }
                Err(message) => {
                    toasts.error(format!("No se pudo exportar: {message}"));
                    error.set(Some(message));
                }
            }
        });
    };

    view! {
        <Dialog>
            <DialogContent open=open class="max-w-xl">
                <DialogHeader>
                    <DialogTitle>"Exportar fixtures"</DialogTitle>
                    <DialogDescription>
                        "Guarda los cambios de " {move || doctype.get()}
                        " en el directorio de una app, para que viajen con ella."
                    </DialogDescription>
                </DialogHeader>

                <Show when=move || error.get().is_some()>
                    <p
                        role="alert"
                        data-name="ExportCustomizationError"
                        class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
                    >
                        {move || error.get().unwrap_or_default()}
                    </p>
                </Show>

                <Show when=move || written.get().is_some()>
                    <p
                        role="status"
                        data-name="ExportCustomizationPath"
                        class="flex items-center gap-2 rounded-md border bg-muted/40 px-3 py-2 text-sm"
                    >
                        <Icon name="check" class="size-4 shrink-0 text-success" />
                        <span class="min-w-0 break-all font-mono text-xs">
                            {move || written.get().unwrap_or_default()}
                        </span>
                    </p>
                </Show>

                <SelectionScope
                    scope=scope
                    loading=loading
                    customization=customization
                    picked_fields=picked_fields
                    picked_properties=picked_properties
                />

                <Separator />

                <div class="flex flex-col gap-1.5">
                    <span class="text-sm font-medium">"App"</span>
                    <Input
                        id="export-customization-app"
                        r#type=InputType::Text
                        placeholder="Nombre de la app"
                        aria_label="Nombre de la app"
                        prop:value=move || app.get()
                        on_input=Callback::new(move |text: String| app.set(text))
                    />
                    <span class="text-xs text-muted-foreground">
                        "Se escribe en plugins/<app>/customizations/, dentro del mundo de la app."
                    </span>
                </div>

                <DialogFooter>
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        on:click=move |_| open.set(false)
                    >
                        "Cancelar"
                    </Button>
                    <Button
                        variant=ButtonVariant::Default
                        size=ButtonSize::Sm
                        attr:disabled=move || exporting.get() || loading.get()
                        on:click=move |_| submit()
                    >
                        {move || if exporting.get() { "Exportando…" } else { "Exportar" }}
                    </Button>
                </DialogFooter>
            </DialogContent>
        </Dialog>
    }
}

/// The scope choice plus, for "cambios concretos", the tickable list.
#[component]
fn SelectionScope(
    scope: RwSignal<String>,
    loading: RwSignal<bool>,
    customization: RwSignal<Option<Customization>>,
    picked_fields: RwSignal<Vec<String>>,
    picked_properties: RwSignal<Vec<(Option<String>, String)>>,
) -> impl IntoView {
    view! {
        <RadioGroup value=scope>
            <div class="flex items-center gap-2">
                <RadioGroupItem value=SCOPE_ALL.to_string() id="export-scope-all" />
                <span class="text-sm">"Todo (por defecto)"</span>
            </div>
            <div class="flex items-center gap-2">
                <RadioGroupItem value=SCOPE_SOME.to_string() id="export-scope-some" />
                <span class="text-sm">"Cambios concretos"</span>
            </div>
        </RadioGroup>

        <Show when=move || scope.get() == SCOPE_SOME>
            <div class="flex max-h-64 flex-col gap-3 overflow-y-auto rounded-lg border p-2">
                <Show
                    when=move || !loading.get()
                    fallback=|| view! {
                        <p class="px-2 py-3 text-sm text-muted-foreground">"Cargando cambios…"</p>
                    }
                >
                    <FieldPicker picked=picked_fields customization=customization />
                    <PropertyPicker picked=picked_properties customization=customization />
                    <Show when=move || {
                        customization
                            .get()
                            .is_none_or(|custom| custom.fields.is_empty() && custom.properties.is_empty())
                    }>
                        <p class="px-2 py-3 text-sm text-muted-foreground">
                            "Esta doctype no tiene cambios que exportar."
                        </p>
                    </Show>
                </Show>
            </div>
        </Show>
    }
}

/// One checkbox row of the scope list, sharing the toggle bookkeeping.
fn checkbox_row(
    label: String,
    hint: String,
    checked: Signal<bool>,
    on_change: Callback<bool>,
) -> impl IntoView {
    // `view!` builds the children before the attributes, so the accessible name
    // needs its own copy of the label.
    let aria_label = label.clone();
    view! {
        <div class="flex items-start gap-2 rounded-md px-2 py-1.5 hover:bg-accent">
            <Checkbox
                checked=checked
                aria_label=aria_label
                on_checked_change=on_change
            />
            <span class="min-w-0">
                <span class="block truncate text-sm">{label}</span>
                <span class="block truncate text-xs text-muted-foreground">{hint}</span>
            </span>
        </div>
    }
}

/// The custom fields the user can pick.
#[component]
fn FieldPicker(
    picked: RwSignal<Vec<String>>,
    customization: RwSignal<Option<Customization>>,
) -> impl IntoView {
    view! {
        <div class="flex flex-col">
            <span class="px-2 py-1 text-xs font-semibold text-muted-foreground">
                "Campos personalizados"
            </span>
            {move || {
                let fields = customization
                    .get()
                    .map(|custom| custom.fields)
                    .unwrap_or_default();
                fields
                    .into_iter()
                    .map(|field| {
                        let fieldname = field.fieldname;
                        let label = field
                            .label
                            .filter(|label| !label.trim().is_empty())
                            .unwrap_or_else(|| fieldname.clone());
                        let name_for_check = fieldname.clone();
                        let name_for_toggle = fieldname.clone();
                        let checked: Signal<bool> = Signal::from(Memo::new(move |_| {
                            picked.get().contains(&name_for_check)
                        }));
                        checkbox_row(
                            label,
                            fieldname,
                            checked,
                            Callback::new(move |checked: bool| {
                                toggle_pick(&picked, &name_for_toggle, checked);
                            }),
                        )
                    })
                    .collect_view()
            }}
        </div>
    }
}

/// The property setters the user can pick.
#[component]
fn PropertyPicker(
    picked: RwSignal<Vec<(Option<String>, String)>>,
    customization: RwSignal<Option<Customization>>,
) -> impl IntoView {
    view! {
        <div class="flex flex-col">
            <span class="px-2 py-1 text-xs font-semibold text-muted-foreground">
                "Propiedades"
            </span>
            {move || {
                let properties = customization
                    .get()
                    .map(|custom| custom.properties)
                    .unwrap_or_default();
                properties
                    .into_iter()
                    .map(|setter| {
                        let PropertySetterSpec {
                            fieldname,
                            property,
                            value,
                        } = setter;
                        let key = (fieldname.clone(), property.clone());
                        let label = format!("{property} = {value}");
                        let hint = fieldname
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| "(doctype)".to_string());
                        let key_for_check = key.clone();
                        let checked: Signal<bool> = Signal::from(Memo::new(move |_| {
                            picked.get().contains(&key_for_check)
                        }));
                        checkbox_row(
                            label,
                            hint,
                            checked,
                            Callback::new(move |checked: bool| {
                                toggle_pick(&picked, &key, checked);
                            }),
                        )
                    })
                    .collect_view()
            }}
        </div>
    }
}

/// Add or remove `value` from `picked`, keeping the selection order stable.
fn toggle_pick<T: Clone + PartialEq + Send + Sync + 'static>(
    picked: &RwSignal<Vec<T>>,
    value: &T,
    checked: bool,
) {
    picked.update(|current| {
        if checked {
            if !current.contains(value) {
                current.push(value.clone());
            }
        } else {
            current.retain(|item| item != value);
        }
    });
}

/// Write the customization into `app`'s plugin world.
///
/// One line on purpose: `scope` is the API's optional filter — `None` exports
/// every change of the DocType, `Some(ExportScope::…)` only the picked ones —
/// so this is the single call site to look at when the contract moves.
async fn export(doctype: &str, app: &str, scope: Option<ExportScope>) -> Result<String, String> {
    let store = DocStore::runtime()?;
    store.export_customization(doctype, app, scope).await
}
