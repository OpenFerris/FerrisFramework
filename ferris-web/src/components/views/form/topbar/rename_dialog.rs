//! Rename dialog: move a document's primary key.
//!
//! The engine owns the rules (`allow_rename`, name validation, target
//! collision); this dialog only asks for the new name and, on success, follows
//! the document to its new route. The current URL holds the old name, so
//! staying on it would show a 404 on the next reload.
//!
//! The outcome is reported twice on purpose: inline (`role="alert"`, kept while
//! the dialog is open so the server's message can be read) and as a transient
//! toast, so the confirmation is still visible after the dialog closes.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;

use crate::api::DocStore;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{
    Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
};
use crate::components::ui::input::{Input, InputType};
use crate::views::services::FormService;

use super::toast::Toasts;

#[component]
pub fn RenameDialog(open: RwSignal<bool>, service: FormService) -> impl IntoView {
    let toasts = Toasts::from_context();
    let doctype = service.doctype;
    let docname = service.docname;
    let module = service.module;
    let new_name = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let saving = RwSignal::new(false);
    // Captured in the component body: `use_navigate` reads the router context,
    // which is not available inside the task after the request resolves.
    let navigate = use_navigate();

    // Seed with the current name every time the dialog opens: a cancelled edit
    // must not leak into the next one.
    Effect::new(move |_| {
        if open.get() {
            new_name.set(docname.get());
            error.set(None);
        }
    });

    let submit = move || {
        let next = new_name.get_untracked().trim().to_string();
        let old = docname.get_untracked();
        if next.is_empty() || next == old {
            open.set(false);
            return;
        }
        let dt = doctype.get_untracked();
        let url = format!(
            "/desk/{}/{}/{}",
            crate::desk::session::path_segment(&module.get_untracked()),
            crate::desk::session::path_segment(&dt),
            crate::desk::session::path_segment(&next)
        );
        error.set(None);
        saving.set(true);
        let navigate = navigate.clone();
        let toasts = toasts.clone();
        spawn_local(async move {
            let result = match DocStore::runtime() {
                Ok(store) => store.rename(&dt, &old, &next).await.map(|_| ()),
                Err(message) => Err(message),
            };
            saving.set(false);
            match result {
                Ok(()) => {
                    toasts.success(format!("Documento renombrado a {next}."));
                    open.set(false);
                    navigate(&url, NavigateOptions::default());
                }
                Err(message) => {
                    toasts.error(format!("No se pudo renombrar: {message}"));
                    error.set(Some(message));
                }
            }
        });
    };

    view! {
        <Dialog>
            <DialogContent open=open class="max-w-md">
                <DialogHeader>
                    <DialogTitle>"Renombrar documento"</DialogTitle>
                    <DialogDescription>
                        "Cambia el nombre del documento. Las referencias y el historial se mueven con él."
                    </DialogDescription>
                </DialogHeader>

                <Show when=move || error.get().is_some()>
                    <p
                        class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
                        role="alert"
                    >
                        {move || error.get().unwrap_or_default()}
                    </p>
                </Show>

                <Input
                    id="rename-doc-name"
                    r#type=InputType::Text
                    placeholder="Nuevo nombre"
                    prop:value=move || new_name.get()
                    on_input=Callback::new(move |text: String| new_name.set(text))
                />

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
                        attr:disabled=move || saving.get()
                        on:click=move |_| submit()
                    >
                        {move || if saving.get() { "Renombrando…" } else { "Renombrar" }}
                    </Button>
                </DialogFooter>
            </DialogContent>
        </Dialog>
    }
}
