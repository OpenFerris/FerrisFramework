//! The confirmation shown when the layout editor is left with a live session.
//!
//! A layout edit is applied the moment it is written (`Custom Field` /
//! `Property Setter`), so leaving loses no work — but it *does* end the undo
//! history of that session, which is the only record of what this client
//! changed. The dialog says exactly that instead of pretending there are
//! unsaved changes to discard, and it is the one place that resolves the
//! pending target: the menu, a breadcrumb and the browser's unload all funnel
//! into the same signal pair.
//!
//! "Cancelar" (and ESC, and the backdrop) only closes the dialog: the caller
//! keeps its stacks and stays in the editor.

use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;

use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{
    Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
};
use crate::views::services::FormService;

/// The leave confirmation of the layout editor.
#[component]
pub(super) fn LeaveLayoutDialog(
    /// Whether the dialog is open; written by the exit paths and by the dialog
    /// itself when it closes.
    open: RwSignal<bool>,
    /// The URL "Salir" navigates to, resolved by whoever opened the dialog.
    target: RwSignal<String>,
    service: FormService,
) -> impl IntoView {
    // `StoredValue` is `Copy`: the confirm handler and the cancel handler are
    // built at different times and both need the service.
    let service = StoredValue::new(service);
    let navigate = use_navigate();

    let cancel = Callback::new(move |()| open.set(false));
    let confirm = Callback::new(move |()| {
        // End the session *before* navigating: the target view must never see
        // the stacks, even for the frame between the route change and the
        // service's own cleanup effect.
        let _ = service.try_with_value(FormService::discard_layout_history);
        open.set(false);
        let url = target.get_untracked();
        if !url.is_empty() {
            navigate(&url, NavigateOptions::default());
        }
    });

    view! {
        <Dialog>
            <DialogContent
                open=open
                class="max-w-md"
                show_close_button=false
                close_on_backdrop_click=false
            >
                <DialogHeader>
                    <DialogTitle>"Salir del editor de diseño"</DialogTitle>
                    <DialogDescription>
                        "Vas a salir del editor de diseño. Los cambios ya se han aplicado al DocType \
                         y se conservan; al salir se cierra la sesión de edición y su historial de \
                         deshacer."
                    </DialogDescription>
                </DialogHeader>
                <DialogFooter class="mt-4">
                    <Button
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        on:click=move |_| cancel.run(())
                    >
                        "Cancelar"
                    </Button>
                    <Button
                        variant=ButtonVariant::Default
                        size=ButtonSize::Sm
                        on:click=move |_| confirm.run(())
                    >
                        "Salir"
                    </Button>
                </DialogFooter>
            </DialogContent>
        </Dialog>
    }
}
