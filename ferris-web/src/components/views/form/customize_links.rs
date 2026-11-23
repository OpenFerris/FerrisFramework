//! Customize → Actions and Links.
//!
//! Frappe's Customize Form can add and remove `DocType Action` and
//! `DocType Link` rows, saved with `custom = 1`
//! (<https://docs.frappe.io/framework/user/en/basics/doctypes/customize>). This
//! component is that surface in the Desk: the rows are ordinary documents, so
//! the form validates them and the ordinary permission rules apply.
//!
//! After every write the metadata cache is cleared and the form reloads, so the
//! new button or connection appears immediately instead of on the next visit.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::icon::Icon;
use crate::components::ui::input::Input;
use crate::components::ui::select_native::SelectNative;
use crate::components::ui::separator::Separator;
use crate::i18n::{t, use_i18n};
use crate::views::services::FormService;

#[component]
pub fn CustomizeActionsLinks(service: FormService) -> impl IntoView {
    let i18n = use_i18n();
    let action_i18n = i18n;
    let link_i18n = i18n;
    let remove_i18n = i18n;
    let doctype = service.doctype;
    let meta = service.meta;
    let error = RwSignal::new(None::<String>);
    let notice = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    // Add-action form.
    let action_label = RwSignal::new(String::new());
    let action_type = RwSignal::new("Server Action".to_string());
    let action_value = RwSignal::new(String::new());
    let action_group = RwSignal::new(String::new());

    // Add-link form.
    let link_doctype = RwSignal::new(String::new());
    let link_field = RwSignal::new(String::new());
    let link_table = RwSignal::new(String::new());
    let link_group = RwSignal::new(String::new());

    let refresh = Callback::new(move |()| {
        crate::api::client::clear_meta_cache();
        service.reload();
    });

    let add_action = move || {
        let label = action_label.get().trim().to_string();
        let action = action_value.get().trim().to_string();
        if label.is_empty() || action.is_empty() {
            error.set(Some(
                action_i18n.t("components.form.customize.action_required"),
            ));
            return;
        }
        let dt = doctype.get_untracked();
        let action_type = action_type.get_untracked();
        let group = action_group.get_untracked();
        busy.set(true);
        error.set(None);
        notice.set(None);
        let i18n = action_i18n;
        spawn_local(async move {
            let result = match DocStore::runtime() {
                Ok(store) => {
                    store
                        .create(
                            "DocType Action",
                            serde_json::json!({
                                "document_type": dt,
                                "label": label,
                                "action_type": action_type,
                                "action": action,
                                "group": (!group.trim().is_empty()).then_some(group),
                                "hidden": 0,
                                "custom": 1,
                            }),
                        )
                        .await
                }
                Err(message) => Err(message),
            };
            busy.set(false);
            match result {
                Ok(_) => {
                    notice.set(Some(i18n.t("components.form.customize.action_added")));
                    action_label.set(String::new());
                    action_value.set(String::new());
                    action_group.set(String::new());
                    refresh.run(());
                }
                Err(message) => error.set(Some(message)),
            }
        });
    };

    let add_link = move || {
        let target = link_doctype.get().trim().to_string();
        let field = link_field.get().trim().to_string();
        if target.is_empty() || field.is_empty() {
            error.set(Some(link_i18n.t("components.form.customize.link_required")));
            return;
        }
        let dt = doctype.get_untracked();
        let table = link_table.get_untracked();
        let group = link_group.get_untracked();
        busy.set(true);
        error.set(None);
        notice.set(None);
        let i18n = link_i18n;
        spawn_local(async move {
            let result = match DocStore::runtime() {
                Ok(store) => {
                    store
                        .create(
                            "DocType Link",
                            serde_json::json!({
                                "document_type": dt,
                                "link_doctype": target,
                                "link_fieldname": field,
                                "table_fieldname": (!table.trim().is_empty()).then_some(table),
                                "group": (!group.trim().is_empty()).then_some(group),
                                "hidden": 0,
                                "custom": 1,
                            }),
                        )
                        .await
                }
                Err(message) => Err(message),
            };
            busy.set(false);
            match result {
                Ok(_) => {
                    notice.set(Some(i18n.t("components.form.customize.link_added")));
                    link_doctype.set(String::new());
                    link_field.set(String::new());
                    link_table.set(String::new());
                    link_group.set(String::new());
                    refresh.run(());
                }
                Err(message) => error.set(Some(message)),
            }
        });
    };

    let remove = Callback::new(move |(kind, name): (String, String)| {
        busy.set(true);
        error.set(None);
        notice.set(None);
        let i18n = remove_i18n;
        spawn_local(async move {
            let result = match DocStore::runtime() {
                Ok(store) => store.delete(&kind, &name).await,
                Err(message) => Err(message),
            };
            busy.set(false);
            match result {
                Ok(()) => {
                    notice.set(Some(i18n.t("components.form.customize.removed")));
                    refresh.run(());
                }
                Err(message) => error.set(Some(message)),
            }
        });
    });

    view! {
        <Separator />
        <section class="flex flex-col gap-3">
            <h3 class="text-sm font-semibold">{t("components.form.customize.title")}</h3>
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

            // ---- Actions ----
            <div class="flex flex-col gap-2">
                <h4 class="text-xs font-semibold uppercase text-muted-foreground">{t("components.form.sidebar.actions")}</h4>
                <Show
                    when=move || meta.get().is_some_and(|meta| !meta.actions.is_empty())
                    fallback=|| view! {
                        <p class="text-sm text-muted-foreground">{t("components.form.customize.none_actions")}</p>
                    }
                >
                    {move || {
                        meta.get()
                            .map(|meta| meta.actions)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|action| {
                                let name = action.name.clone();
                                let label = if action.label.is_empty() {
                                    action.action.clone()
                                } else {
                                    action.label.clone()
                                };
                                view! {
                                    <div class="flex min-w-0 items-center justify-between gap-2 text-sm">
                                        <span class="min-w-0 truncate">
                                            <span class="font-medium">{label}</span>
                                            <span class="ml-2 text-xs text-muted-foreground">
                                                {action.action_type.clone()}
                                            </span>
                                        </span>
                                        <Button
                                            variant=ButtonVariant::Ghost
                                            size=ButtonSize::IconSm
                                            class="shrink-0"
                                            attr:aria-label=t("components.form.customize.remove_action")
                                            on:click=move |_| {
                                                remove.run((
                                                    "DocType Action".to_string(),
                                                    name.clone(),
                                                ));
                                            }
                                        >
                                            <Icon name="x" />
                                        </Button>
                                    </div>
                                }
                            })
                            .collect_view()
                    }}
                </Show>
                <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
                    <Input placeholder=t("components.form.customize.label_placeholder") bind_value=action_label />
                    <SelectNative
                        id="customize-action-type"
                        aria_label=t("components.form.customize.action_type")
                        value=action_type.read_only()
                        on_change=Callback::new(move |ev: leptos::ev::Event| {
                            action_type.set(event_target_value(&ev));
                        })
                    >
                        <option value="Server Action">"Server Action"</option>
                        <option value="Route">"Route"</option>
                    </SelectNative>
                    <Input placeholder=t("components.form.customize.method_placeholder") bind_value=action_value />
                    <Input placeholder=t("components.form.customize.group_placeholder") bind_value=action_group />
                </div>
                <Button
                    variant=ButtonVariant::Secondary
                    size=ButtonSize::Sm
                    attr:disabled=busy
                    on:click=move |_| add_action()
                >
                    {t("components.form.customize.add_action")}
                </Button>
            </div>

            // ---- Links ----
            <div class="flex flex-col gap-2">
                <h4 class="text-xs font-semibold uppercase text-muted-foreground">{t("components.form.customize.links")}</h4>
                <Show
                    when=move || meta.get().is_some_and(|meta| !meta.links.is_empty())
                    fallback=|| view! {
                        <p class="text-sm text-muted-foreground">{t("components.form.customize.none_links")}</p>
                    }
                >
                    {move || {
                        meta.get()
                            .map(|meta| meta.links)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|link| {
                                let name = link.name.clone();
                                view! {
                                    <div class="flex min-w-0 items-center justify-between gap-2 text-sm">
                                        <span class="min-w-0 truncate">
                                            <span class="font-medium">{link.link_doctype.clone()}</span>
                                            <span class="ml-2 text-xs text-muted-foreground">
                                                {link.link_fieldname.clone()}
                                            </span>
                                        </span>
                                        <Button
                                            variant=ButtonVariant::Ghost
                                            size=ButtonSize::IconSm
                                            class="shrink-0"
                                            attr:aria-label=t("components.form.customize.remove_link")
                                            on:click=move |_| {
                                                remove.run((
                                                    "DocType Link".to_string(),
                                                    name.clone(),
                                                ));
                                            }
                                        >
                                            <Icon name="x" />
                                        </Button>
                                    </div>
                                }
                            })
                            .collect_view()
                    }}
                </Show>
                <div class="grid grid-cols-1 gap-2 sm:grid-cols-2">
                    <Input placeholder=t("components.form.customize.target_doctype") bind_value=link_doctype />
                    <Input placeholder=t("components.form.customize.link_field") bind_value=link_field />
                    <Input placeholder=t("components.form.customize.child_table") bind_value=link_table />
                    <Input placeholder=t("components.form.customize.group_placeholder") bind_value=link_group />
                </div>
                <Button
                    variant=ButtonVariant::Secondary
                    size=ButtonSize::Sm
                    attr:disabled=busy
                    on:click=move |_| add_link()
                >
                    {t("components.form.customize.add_link")}
                </Button>
            </div>
        </section>
    }
}
