//! "Personalizar" dialog: the runtime customization surface.
//!
//! The fieldtype catalog and its hints live in the [`types`] submodule; this
//! file is the dialog that writes the customization. `FIELD_TYPES`, `hint` and
//! `nonempty` are re-exported here, so every caller keeps the
//! `customize::…` path it already used.
//!
//! It reads the current customizations (`GET /api/customization/{doctype}`),
//! lets the user add or remove a custom field and a property setter, and writes
//! each change straight to the server. After a successful write the dialog
//! re-reads the customization and asks the form service to reload, so the new
//! field or override appears in the form without a recompilation — which is what
//! "customizable at runtime, persisted on the server" means.
//!
//! The write endpoints require `Manage`; a caller without it gets the server's
//! refusal surfaced here (never a silent no-op).
//!
//! A user picker for the owner/property targets is deliberately absent: this
//! dialog edits *metadata*, and the metadata endpoints take field and property
//! names, not users.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use crate::api::types::{CustomFieldSpec, Customization, PropertySetterSpec};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::dialog::{
    Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle,
};
use crate::components::ui::input::Input;
use crate::components::ui::select_native::SelectNative;
use crate::components::ui::separator::Separator;
use crate::views::services::FormService;

pub(crate) mod types;

// Re-exported so `customize::FIELD_TYPES`/`hint`/`nonempty` keep resolving for
// the field creator, the inspector and the tests.
pub(crate) use types::{FIELD_TYPES, hint, nonempty};

#[component]
pub fn CustomizeDialog(open: RwSignal<bool>, service: FormService) -> impl IntoView {
    let service = StoredValue::new(service);
    let svc = service.get_value();
    let doctype = svc.doctype;

    let custom = RwSignal::new(Customization::default());
    let loading = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let notice = RwSignal::new(None::<String>);

    // Add-field form.
    let fname = RwSignal::new(String::new());
    let flabel = RwSignal::new(String::new());
    let ftype = RwSignal::new("Data".to_string());
    let foptions = RwSignal::new(String::new());
    let fafter = RwSignal::new(String::new());
    let freqd = RwSignal::new(false);

    // Export to a plugin world.
    let export_app = RwSignal::new(String::new());
    let exporting = RwSignal::new(false);

    // Add-property form.
    let pfield = RwSignal::new(String::new());
    let pprop = RwSignal::new(String::new());
    let pvalue = RwSignal::new(String::new());

    // Re-read the customization. A plain `Fn` closure over copy signals, so it
    // can be called from every handler and from the open effect.
    let reload = move || {
        let dt = doctype.get();
        loading.set(true);
        error.set(None);
        spawn_local(async move {
            let store = match DocStore::runtime() {
                Ok(store) => store,
                Err(e) => {
                    error.set(Some(e));
                    loading.set(false);
                    return;
                }
            };
            match store.get_customization(&dt).await {
                Ok(c) => {
                    custom.set(c);
                    loading.set(false);
                }
                Err(e) => {
                    error.set(Some(e));
                    loading.set(false);
                }
            }
        });
    };

    // Load whenever the dialog opens.
    Effect::new(move |_| {
        if open.get() {
            reload();
        }
    });

    // Clear the forms whenever it closes, so a reopened dialog starts blank.
    Effect::new(move |_| {
        if !open.get() {
            fname.set(String::new());
            flabel.set(String::new());
            foptions.set(String::new());
            fafter.set(String::new());
            freqd.set(false);
        }
    });

    let export_to_app = move || {
        let app = export_app.get_untracked().trim().to_string();
        if app.is_empty() {
            error.set(Some("Escribe el nombre de la app.".to_string()));
            return;
        }
        let dt = doctype.get_untracked();
        error.set(None);
        notice.set(None);
        exporting.set(true);
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                exporting.set(false);
                return;
            };
            // The dialog exports the doctype's whole customization; the scoped
            // variants belong to the field editor (`edit_mode::export`).
            match store.export_customization(&dt, &app, None).await {
                Ok(path) => {
                    exporting.set(false);
                    notice.set(Some(format!("Customización exportada a {path}")));
                }
                Err(message) => {
                    exporting.set(false);
                    error.set(Some(message));
                }
            }
        });
    };

    let submit_field = move || {
        let spec = CustomFieldSpec {
            fieldname: fname.get().trim().to_string(),
            label: nonempty(&flabel.get()),
            fieldtype: ftype.get(),
            options: nonempty(&foptions.get()),
            insert_after: nonempty(&fafter.get()),
            reqd: freqd.get(),
            default: None,
            hidden: false,
            read_only: false,
        };
        if spec.fieldname.is_empty() {
            error.set(Some("El nombre del campo es obligatorio".to_string()));
            return;
        }
        let dt = doctype.get();
        notice.set(None);
        error.set(None);
        let forms = (fname, flabel, foptions, fafter, freqd);
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                error.set(Some("Sin runtime".to_string()));
                return;
            };
            match store.put_custom_field(&dt, &spec).await {
                Ok(saved) => {
                    notice.set(Some(format!("Campo «{}» guardado.", saved.fieldname)));
                    forms.0.set(String::new());
                    forms.1.set(String::new());
                    forms.2.set(String::new());
                    forms.3.set(String::new());
                    forms.4.set(false);
                    reload();
                    service.get_value().reload();
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    let submit_property = move || {
        let property = pprop.get().trim().to_string();
        if property.is_empty() {
            error.set(Some("La propiedad es obligatoria".to_string()));
            return;
        }
        let spec = PropertySetterSpec {
            fieldname: nonempty(&pfield.get()),
            property,
            value: pvalue.get(),
        };
        let dt = doctype.get();
        notice.set(None);
        error.set(None);
        let fields = (pfield, pprop, pvalue);
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                error.set(Some("Sin runtime".to_string()));
                return;
            };
            match store.put_property_setter(&dt, &spec).await {
                Ok(saved) => {
                    notice.set(Some(format!("Propiedad «{}» guardada.", saved.property)));
                    fields.0.set(String::new());
                    fields.1.set(String::new());
                    fields.2.set(String::new());
                    reload();
                    service.get_value().reload();
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    view! {
        <Dialog>
            <DialogContent open=open class="max-w-2xl max-h-[85dvh]">
                <DialogHeader>
                    <DialogTitle>"Personalizar " {move || doctype.get()}</DialogTitle>
                    <DialogDescription>
                        "Los cambios se guardan en el servidor y se aplican al formulario."
                    </DialogDescription>
                </DialogHeader>

                // The field editor lives on the form itself: this dialog keeps
                // the custom fields, actions and links; picking a field's
                // properties happens where the field is.

                // Export: customizations become part of the plugin, not a
                // database-only setting, so they travel with the app.
                <div class="flex flex-col gap-2 rounded-lg border bg-muted/40 p-3">
                    <p class="text-sm font-medium">"Exportar a una app"</p>
                    <p class="text-xs text-muted-foreground">
                        "Guarda los campos y propiedades en plugins/<app>/customizations/ para que viajen con el plugin."
                    </p>
                    <div class="flex flex-wrap items-center gap-2">
                        <Input
                            placeholder="mi-app"
                            bind_value=export_app
                            class="h-9 min-w-40 flex-1"
                        />
                        <Button
                            variant=ButtonVariant::Secondary
                            size=ButtonSize::Sm
                            attr:disabled=move || exporting.get()
                            on:click=move |_| export_to_app()
                        >
                            {move || if exporting.get() { "Exportando…" } else { "Exportar" }}
                        </Button>
                    </div>
                </div>

                <Show when=move || error.get().is_some()>
                    <div class="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
                        {move || error.get().unwrap_or_default()}
                    </div>
                </Show>
                <Show when=move || notice.get().is_some()>
                    <div class="rounded-lg border border-success/40 bg-success/10 px-3 py-2 text-sm text-success">
                        {move || notice.get().unwrap_or_default()}
                    </div>
                </Show>

                <div class="flex max-h-[60dvh] flex-col gap-4 overflow-y-auto pr-1">
                    // ---- Custom fields ----
                    <section class="flex flex-col gap-2">
                        <h3 class="text-sm font-semibold">"Campos personalizados"</h3>
                        <Show
                            when=move || !custom.get().fields.is_empty()
                            fallback=|| view! {
                                <p class="text-sm text-muted-foreground">"Ninguno."</p>
                            }
                        >
                            {move || custom.get().fields.into_iter().map(|field| {
                                let name = field.fieldname.clone();
                                let name_display = name.clone();
                                let label = field.label.clone().unwrap_or(name_display.clone());
                                let ty = field.fieldtype.clone();
                                view! {
                                    <div class="flex min-w-0 items-center justify-between gap-2 rounded-md border px-3 py-2">
                                        <div class="min-w-0">
                                            // `block`: `truncate` needs a block
                                            // box to clip; on an inline span the
                                            // label ran under the delete button
                                            // on a phone.
                                            <span class="block truncate text-sm font-medium">{label}</span>
                                            <span class="block truncate text-xs text-muted-foreground">
                                                {name_display} " · " {ty}
                                            </span>
                                        </div>
                                        <Button
                                            variant=ButtonVariant::Ghost
                                            size=ButtonSize::Sm
                                            class="shrink-0"
                                            on:click=move |_| {
                                                let name = name.clone();
                                                let dt = doctype.get();
                                                spawn_local(async move {
                                                    let Ok(store) = DocStore::runtime() else { return };
                                                    match store.delete_custom_field(&dt, &name).await {
                                                        Ok(()) => {
                                                            notice.set(Some(format!("Campo «{name}» eliminado.")));
                                                            reload();
                                                            service.get_value().reload();
                                                        }
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                });
                                            }
                                        >
                                            "Eliminar"
                                        </Button>
                                    </div>
                                }
                            }).collect_view()}
                        </Show>

                        <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
                            <Input
                                placeholder="nombre_del_campo"
                                bind_value=fname
                                class="h-9"
                            />
                            <Input placeholder="Etiqueta" bind_value=flabel class="h-9" />
                            <SelectNative
                                id="customize-fieldtype"
                                aria_label="Tipo de campo"
                                value=ftype.read_only()
                                on_change=Callback::new(move |ev: leptos::ev::Event| {
                                    ftype.set(event_target_value(&ev));
                                })
                            >
                                {FIELD_TYPES.iter().map(|t| view! {
                                    <option value=*t>{format!("{t} — {}", hint(t))}</option>
                                }).collect_view()}
                            </SelectNative>
                            <Input
                                placeholder="Opciones (para Select/Link)"
                                bind_value=foptions
                                class="h-9"
                            />
                            <Input
                                placeholder="Insertar después de…"
                                bind_value=fafter
                                class="h-9"
                            />
                            <label class="flex items-center gap-2 text-sm">
                                <Checkbox checked=freqd aria_label="Obligatorio" />
                                "Obligatorio"
                            </label>
                        </div>
                        <p class="text-xs text-muted-foreground">
                            {move || hint(&ftype.get())}
                        </p>
                        <Button
                            variant=ButtonVariant::Secondary
                            size=ButtonSize::Sm
                            on:click=move |_| submit_field()
                        >
                            "Añadir campo"
                        </Button>
                    </section>

                    <Separator />

                    // ---- Property setters ----
                    <section class="flex flex-col gap-2">
                        <h3 class="text-sm font-semibold">"Propiedades"</h3>
                        <Show
                            when=move || !custom.get().properties.is_empty()
                            fallback=|| view! {
                                <p class="text-sm text-muted-foreground">"Ninguna."</p>
                            }
                        >
                            {move || custom.get().properties.into_iter().map(|setter| {
                                let field = setter.fieldname.clone();
                                let property = setter.property.clone();
                                let property_display = property.clone();
                                let value = setter.value.clone();
                                let scope = field.clone().unwrap_or_else(|| "(doctype)".to_string());
                                view! {
                                    <div class="flex min-w-0 items-center justify-between gap-2 rounded-md border px-3 py-2">
                                        <div class="min-w-0">
                                            <span class="block truncate text-sm font-medium">{property_display}</span>
                                            <span class="block truncate text-xs text-muted-foreground">
                                                {scope} " = " {value}
                                            </span>
                                        </div>
                                        <Button
                                            variant=ButtonVariant::Ghost
                                            size=ButtonSize::Sm
                                            class="shrink-0"
                                            on:click=move |_| {
                                                let field = field.clone();
                                                let property = property.clone();
                                                let dt = doctype.get();
                                                spawn_local(async move {
                                                    let Ok(store) = DocStore::runtime() else { return };
                                                    match store
                                                        .delete_property_setter(&dt, field.as_deref(), &property)
                                                        .await
                                                    {
                                                        Ok(()) => {
                                                            notice.set(Some(format!("Propiedad «{property}» eliminada.")));
                                                            reload();
                                                            service.get_value().reload();
                                                        }
                                                        Err(e) => error.set(Some(e)),
                                                    }
                                                });
                                            }
                                        >
                                            "Eliminar"
                                        </Button>
                                    </div>
                                }
                            }).collect_view()}
                        </Show>

                        <div class="grid grid-cols-1 gap-2 sm:grid-cols-3">
                            <Input placeholder="campo (vacío = doctype)" bind_value=pfield class="h-9" />
                            <Input placeholder="propiedad (p. ej. reqd)" bind_value=pprop class="h-9" />
                            <Input placeholder="valor" bind_value=pvalue class="h-9" />
                        </div>
                        <Button
                            variant=ButtonVariant::Secondary
                            size=ButtonSize::Sm
                            on:click=move |_| submit_property()
                        >
                            "Añadir propiedad"
                        </Button>
                    </section>

                    <crate::components::views::form::customize_links::CustomizeActionsLinks
                        service=svc.clone()
                    />

                    <Show when=move || loading.get()>
                        <p class="text-xs text-muted-foreground">"Cargando…"</p>
                    </Show>
                </div>
            </DialogContent>
        </Dialog>
    }
}
