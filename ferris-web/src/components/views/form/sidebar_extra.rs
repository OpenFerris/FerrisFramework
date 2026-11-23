//! DocType-driven extras of the form sidebar: Actions and Connections.
//!
//! These two panels are the end-user half of Frappe's *Actions and Links*
//! (<https://docs.frappe.io/framework/user/en/basics/doctypes/actions-and-links>).
//! They are split out of [`super::sidebar`] because that file is already at the
//! repository's size limit; both panels are pure renders over data the sidebar
//! loads, so the separation adds no state and no data path.
//!
//! Actions come from the DocType metadata (`DynMeta::actions`). A `Route`
//! action is a real link; a `Server Action` is rendered inert and labelled,
//! because this build has no whitelisted-method registry and a button that
//! silently does nothing would be worse than one that says so.
//!
//! Connections come from `GET /api/doctype/connections/{doctype}/{name}`; the
//! server already applied the caller's read permission to every document in the
//! payload.

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use crate::api::DynMeta;
use crate::api::types::ConnectionGroup;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::separator::Separator;
use crate::i18n::{t, use_i18n};
use crate::views::services::form_service::{FormField, build_form_payload, strip_virtual_fields};

/// The `DocType Action` buttons of the current DocType.
///
/// A `Route` action is a real link. A `Server Action` calls the whitelisted
/// method through the normal `/api/method/{method}` endpoint with Frappe's
/// `kwargs` shape (`doctype`, `docname`, `doc`); the button reports the
/// server's error instead of swallowing it, and reloads the form after success
/// so any write the method performed is visible immediately.
#[component]
pub fn DocTypeActions(
    meta: RwSignal<Option<DynMeta>>,
    doctype: Signal<String>,
    docname: Signal<String>,
    fields: RwSignal<Vec<FormField>>,
    error: RwSignal<Option<String>>,
    reload: Callback<()>,
) -> impl IntoView {
    let i18n = use_i18n();
    let notice = RwSignal::new(None::<String>);
    view! {
        <Show when=move || {
            meta.get()
                .is_some_and(|meta| meta.actions.iter().any(|action| !action.hidden))
        }>
            <section class="flex flex-col gap-2">
                <h3 class="text-sm font-semibold">{t("components.form.sidebar.actions")}</h3>
                {move || {
                    meta.get()
                        .map(|meta| meta.actions)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|action| !action.hidden)
                        .map(|action| {
                            let action_i18n = i18n;
                            let label = if action.label.is_empty() {
                                action.action.clone()
                            } else {
                                action.label.clone()
                            };
                            if action.action_type == "Route" {
                                view! {
                                    <a
                                        class="text-sm text-primary underline-offset-4 hover:underline"
                                        href=action.action
                                    >
                                        {label}
                                    </a>
                                }
                                .into_any()
                            } else {
                                let busy = RwSignal::new(false);
                                let method = action.action.clone();
                                let invoked = label.clone();
                                view! {
                                    <Button
                                        variant=ButtonVariant::Outline
                                        size=ButtonSize::Sm
                                        attr:aria-busy=move || busy.get().to_string()
                                        attr:disabled=move || {
                                            busy.get() || docname.get().trim().is_empty()
                                        }
                                        on:click=move |_| {
                                            let method = method.clone();
                                            let invoked = invoked.clone();
                                            let i18n = action_i18n;
                                            // The action sees what the user sees,
                                            // including unsaved edits (Frappe
                                            // passes the live document), but a
                                            // computed virtual field is omitted:
                                            // it is read-only and has no storage.
                                            let document = strip_virtual_fields(
                                                build_form_payload(&fields.get_untracked()),
                                                meta.get_untracked().as_ref(),
                                            );
                                            let params = serde_json::json!({
                                                "doctype": doctype.get_untracked(),
                                                "docname": docname.get_untracked(),
                                                "doc": document,
                                            });
                                            busy.set(true);
                                            notice.set(None);
                                            spawn_local(async move {
                                                let result = match DocStore::runtime() {
                                                    Ok(store) => store.call_method(&method, params).await,
                                                    Err(message) => Err(message),
                                                };
                                                busy.set(false);
                                                match result {
                                                    Ok(_) => {
                                                        error.set(None);
                                                        notice.set(Some(i18n.t_with(
                                                            "components.form.action.ran",
                                                            &[("label", &invoked)],
                                                        )));
                                                        reload.run(());
                                                    }
                                                    Err(message) => {
                                                        notice.set(None);
                                                        error.set(Some(message));
                                                    }
                                                }
                                            });
                                        }
                                    >
                                        {label}
                                    </Button>
                                }
                                .into_any()
                            }
                        })
                        .collect_view()
                }}
                <Show when=move || notice.get().is_some()>
                    <p class="text-xs text-muted-foreground">{move || notice.get().unwrap_or_default()}</p>
                </Show>
            </section>
        </Show>
    }
}

/// The Connections panel of the current document.
#[component]
pub fn ConnectionsPanel(
    connections: RwSignal<Vec<ConnectionGroup>>,
    error: RwSignal<Option<String>>,
    modules: RwSignal<HashMap<String, String>>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <Show when=move || !connections.get().is_empty() || error.get().is_some()>
            <Separator />
            <section class="flex flex-col gap-2">
                <h3 class="text-sm font-semibold">{t("components.form.sidebar.connections")}</h3>
                <Show when=move || error.get().is_some()>
                    <p class="text-xs text-destructive">{move || error.get().unwrap_or_default()}</p>
                </Show>
                {move || {
                    let module_of = modules.get();
                    connections
                        .get()
                        .into_iter()
                        .map(|group| {
                            let heading = group
                                .group
                                .clone()
                                .unwrap_or_else(|| group.link_doctype.clone());
                            let link_doctype = group.link_doctype.clone();
                            let module = module_of
                                .get(&link_doctype)
                                .cloned()
                                .unwrap_or_else(|| link_doctype.clone());
                            let count = group.count;
                            let documents = group
                                .documents
                                .into_iter()
                                .map(|document| {
                                    let href = format!(
                                        "/desk/{}/{}/{}",
                                        crate::desk::session::path_segment(&module),
                                        crate::desk::session::path_segment(&link_doctype),
                                        crate::desk::session::path_segment(&document.name)
                                    );
                                    let title = document
                                        .title
                                        .filter(|title| !title.is_empty())
                                        .unwrap_or_else(|| document.name.clone());
                                    view! {
                                        <a
                                            class="truncate text-sm text-primary underline-offset-4 hover:underline"
                                            href=href
                                        >
                                            {title}
                                        </a>
                                    }
                                })
                                .collect_view();
                            view! {
                                <div class="flex flex-col gap-1">
                                    <p class="text-xs text-muted-foreground">
                                        {i18n.t_with(
                                            "components.form.sidebar.connections_group",
                                            &[
                                                ("heading", &heading),
                                                ("count", &count.to_string()),
                                            ],
                                        )}
                                    </p>
                                    {documents}
                                </div>
                            }
                        })
                        .collect_view()
                }}
            </section>
        </Show>
    }
}
