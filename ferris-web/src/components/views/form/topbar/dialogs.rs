//! The dialogs the form's "⋯" menu opens, plus the browser helpers they use.

use leptos::prelude::*;
use leptos_router::components::A;
use wasm_bindgen::JsCast;

use crate::components::ui::dialog::{Dialog, DialogContent, DialogHeader, DialogTitle};
use crate::i18n::t;
use crate::views::services::FormField;

/// The "Ir a campo" dialog: the form's jumpable fields, each scrolling to its
/// input. Frappe's own picker, kept to the same shape.
#[component]
pub(super) fn JumpToFieldDialog(
    open: RwSignal<bool>,
    fields: RwSignal<Vec<FormField>>,
) -> impl IntoView {
    view! {
        <Dialog class="max-w-md">
            <DialogContent open=open>
                <DialogHeader>
                    <DialogTitle>{t("components.form.menu.jump_to_field")}</DialogTitle>
                </DialogHeader>
                <div class="flex max-h-80 flex-col gap-1 overflow-y-auto">
                    {move || fields.get().into_iter().filter(|f| is_jumpable(&f.fieldtype)).map(|f| {
                        let fieldname = f.fieldname.clone();
                        view! {
                            <button
                                type="button"
                                class="flex items-center justify-between rounded-md px-3 py-2 text-left text-sm outline-none hover:bg-accent focus-visible:ring-1 focus-visible:ring-ring"
                                on:click=move |_| {
                                    focus_field(&fieldname);
                                    open.set(false);
                                }
                            >
                                <span class="truncate">{f.label}</span>
                                <span class="ml-2 shrink-0 text-xs text-muted-foreground">
                                    {f.fieldname}
                                </span>
                            </button>
                        }
                    }).collect_view()}
                </div>
            </DialogContent>
        </Dialog>
    }
}

/// Whether a field can be jumped to: the layout breaks and the read-only
/// renderers have no input to focus.
fn is_jumpable(fieldtype: &str) -> bool {
    !matches!(
        fieldtype,
        "Tab Break" | "Section Break" | "Column Break" | "HTML" | "Table"
    )
}

/// The "Mostrar vínculos" dialog: the document's links, each a route.
#[component]
pub(super) fn LinkedDocumentsDialog(
    open: RwSignal<bool>,
    module: Signal<String>,
    linked: Memo<Vec<(String, String, String)>>,
) -> impl IntoView {
    view! {
        <Dialog class="max-w-md">
            <DialogContent open=open>
                <DialogHeader>
                    <DialogTitle>{t("components.form.linked_docs.title")}</DialogTitle>
                </DialogHeader>
                <div class="flex max-h-80 flex-col gap-1 overflow-y-auto">
                    <Show
                        when=move || !linked.get().is_empty()
                        fallback=|| view! {
                            <p class="py-4 text-center text-sm text-muted-foreground">
                                {t("components.form.linked_docs.empty")}
                            </p>
                        }
                    >
                        {move || linked.get().into_iter().map(|(label, target, value)| {
                            let href = format!("/desk/{}/{}/{}", module.get(), target, value);
                            view! {
                                <A
                                    href=href
                                    attr:class="flex items-center justify-between rounded-md px-3 py-2 text-sm outline-none hover:bg-accent focus-visible:ring-1 focus-visible:ring-ring"
                                >
                                    <span class="truncate">{label}</span>
                                    <span class="ml-2 shrink-0 text-xs text-muted-foreground">
                                        {target} "/" {value}
                                    </span>
                                </A>
                            }
                        }).collect_view()}
                    </Show>
                </div>
            </DialogContent>
        </Dialog>
    }
}

/// Focus and scroll to a form field, by its DOM id.
///
/// A no-op outside the browser (SSR), where there is no document to focus.
fn focus_field(fieldname: &str) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(element) = document.get_element_by_id(&format!("field-{fieldname}")) else {
        return;
    };
    let Ok(element) = element.dyn_into::<web_sys::HtmlElement>() else {
        return;
    };
    let _ = element.focus();
    element.scroll_into_view();
}

/// Copy `text` to the clipboard, when the browser allows it.
pub(super) fn copy_to_clipboard(text: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let _ = window.navigator().clipboard().write_text(text);
}
