//! Export one field's customization into a plugin world.
//!
//! A customization that lives only in the database is invisible to the next
//! deployment. Exporting writes the `Custom Field`/`Property Setter` payload to
//! `plugins/<app>/customizations/<doctype>.json`, the fixture `ferris migrate`
//! replays, so the runtime edit travels with the app that owns the form.
//!
//! The whole-DocType export is the top bar's **Exportar fixtures** dialog,
//! which already offers every change (or a ticked subset); this one is the
//! field-scoped shortcut the properties header exposes.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use crate::api::types::ExportScope;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{
    Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle,
};
use crate::components::ui::input::{Input, InputType};
use crate::i18n::{t, use_i18n};

/// The export dialog for a single field. `doctype` is read when the export
/// runs, not when the dialog is built, because the same route serves every
/// doctype.
#[component]
pub(super) fn ExportDialog(
    open: RwSignal<bool>,
    doctype: Signal<String>,
    fieldname: String,
) -> impl IntoView {
    let i18n = use_i18n();
    let description_i18n = i18n;
    let export_i18n = i18n;
    let exported_i18n = i18n;
    let app = RwSignal::new(String::new());
    let exporting = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let exported = RwSignal::new(None::<String>);
    let field = StoredValue::new(fieldname);

    let title = t("components.form.export_field.title");
    let description = move || {
        description_i18n.t_with(
            "components.form.export_field.description",
            &[("field", &field.get_value())],
        )
    };

    // A closed dialog starts clean: a path or an error from the previous export
    // must not greet the next one.
    Effect::new(move |_| {
        if !open.get() {
            error.set(None);
            exported.set(None);
            exporting.set(false);
        }
    });

    let export = Callback::new(move |()| {
        let app_name = app.get_untracked().trim().to_string();
        if app_name.is_empty() {
            error.set(Some(export_i18n.t("components.form.export.app_required")));
            return;
        }
        let dt = doctype.get_untracked();
        let fieldname = field.get_value();
        error.set(None);
        exported.set(None);
        exporting.set(true);
        let i18n = export_i18n;
        spawn_local(async move {
            let Ok(store) = DocStore::runtime() else {
                exporting.set(false);
                error.set(Some(i18n.t("components.form.export.unavailable")));
                return;
            };
            // `Some(Fields(..))` narrows the file to this field; `None` (the
            // whole doctype) is the top bar dialog's job.
            let result = store
                .export_customization(&dt, &app_name, Some(ExportScope::Fields(vec![fieldname])))
                .await;
            exporting.set(false);
            match result {
                Ok(path) => exported.set(Some(path)),
                Err(message) => error.set(Some(message)),
            }
        });
    });

    view! {
        <Dialog>
            <DialogContent open=open class="max-w-md">
                <DialogHeader>
                    <DialogTitle>{title}</DialogTitle>
                    <DialogDescription>{description}</DialogDescription>
                </DialogHeader>

                <div class="mt-4 flex flex-col gap-3">
                    <div class="flex flex-col gap-1">
                        <span class="text-xs font-medium text-muted-foreground">{t("components.form.export.app")}</span>
                        <Input
                            r#type=InputType::Text
                            aria_label=t("components.form.export.app_placeholder")
                            placeholder="mi-app"
                            bind_value=app
                        />
                    </div>

                    <Show when=move || error.get().is_some()>
                        <div
                            class="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
                            role="alert"
                        >
                            {move || error.get().unwrap_or_default()}
                        </div>
                    </Show>
                    <Show when=move || exported.get().is_some()>
                        <div
                            class="rounded-lg border border-success/40 bg-success/10 px-3 py-2 text-sm text-success"
                            role="status"
                        >
                            {move || {
                                exported
                                    .get()
                                    .map(|path| {
                                        exported_i18n.t_with(
                                            "components.form.export_field.exported_to",
                                            &[("path", &path)],
                                        )
                                    })
                                    .unwrap_or_default()
                            }}
                        </div>
                    </Show>

                    <div class="flex justify-end gap-2">
                        <Button
                            variant=ButtonVariant::Ghost
                            size=ButtonSize::Sm
                            on:click=move |_| open.set(false)
                        >
                            {t("common.close")}
                        </Button>
                        <Button
                            variant=ButtonVariant::Default
                            size=ButtonSize::Sm
                            attr:disabled=move || exporting.get()
                            on:click=move |_| export.run(())
                        >
                            {move || {
                                if exporting.get() {
                                    t("components.form.export.exporting")
                                } else {
                                    t("components.form.export.export")
                                }
                            }}
                        </Button>
                    </div>
                </div>
            </DialogContent>
        </Dialog>
    }
}
