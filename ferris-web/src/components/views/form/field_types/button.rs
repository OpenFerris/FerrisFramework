//! Action (`Button`): a button that runs the `/api/method/<options>` method
//! against the current document — the Frappe `Button` field, where `options`
//! names the action.
//!
//! Nothing is invented client-side: the server must have registered the
//! method (`ferris-api`'s `/api/method/{method}` route), and a refusal is
//! surfaced in the form's own notice banner instead of being swallowed. On a
//! `new` form there is no document to act on, so the button is disabled and
//! says why.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::json;

use super::doc_context::FormDocContext;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};

#[component]
pub fn ButtonField(id: String, label: String, method: String) -> impl IntoView {
    let ctx = use_context::<FormDocContext>();
    let pending = RwSignal::new(false);
    let label_for_note = label.clone();

    view! {
        <Button
            variant=ButtonVariant::Default
            size=ButtonSize::Sm
            attr:id=id
            attr:data-name="__ButtonField"
            attr:disabled=move || ctx.is_none_or(|c| c.is_new.get() || pending.get())
            attr:title=move || {
                ctx.and_then(|c| {
                    c.is_new
                        .get()
                        .then(|| "Guarda el documento antes de ejecutar esta acción.".to_string())
                })
            }
            on:click=move |_| {
                let Some(ctx) = ctx else { return };
                if ctx.is_new.get() {
                    ctx.notice.set(Some(
                        "Guarda el documento antes de ejecutar esta acción.".to_string(),
                    ));
                    return;
                }
                let method = method.clone();
                let label = label_for_note.clone();
                let doctype = ctx.doctype.get();
                let name = ctx.docname.get();
                let doc = ctx.doc.get();
                pending.set(true);
                spawn_local(async move {
                    let body = json!({ "doctype": doctype, "name": name, "doc": doc }).to_string();
                    let path = format!("/api/method/{method}");
                    match crate::api::client::api_json("POST", &path, Some(&body)).await {
                        Ok(_) => {
                            ctx.notice
                                .set(Some(format!("Acción «{label}» ejecutada.")));
                            ctx.reload.update(|tick| *tick += 1);
                        }
                        Err(e) => ctx.notice.set(Some(format!("Acción «{label}»: {e}"))),
                    }
                    pending.set(false);
                });
            }
        >
            {label}
        </Button>
    }
}
