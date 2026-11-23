//! Role Permissions Manager: custom permission types per role.
//!
//! Frappe's Permission Types (<https://docs.frappe.io/framework/permission-types>)
//! are declared per DocType and then granted to roles from the Role Permission
//! Manager. This page is that manager's custom-permission half:
//!
//! 1. the operator types a DocType and loads its declared types and roles;
//! 2. every cell is a checkbox backed by `Custom DocPerm.custom_permissions`;
//! 3. a change rebuilds the installed policy on the server immediately.
//!
//! The endpoint is System Manager only. A non-admin sees the server's refusal,
//! never an empty matrix that looks like "nothing is granted".

use leptos::prelude::*;

use crate::api;
use crate::api::DocStore;
use crate::api::types::PermissionMatrix;
use crate::app::doctype_display;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::card::{Card, CardContent, CardDescription, CardHeader, CardTitle};
use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::select_native::SelectNative;
use crate::components::ui::table as t;
use crate::i18n::use_i18n;
use crate::views::views_components::{Feedback, FeedbackKind};

#[component]
pub fn CustomPermissionManager() -> impl IntoView {
    let i18n = use_i18n();
    let tr = move |key: &str| {
        i18n.try_with_value(|i| i.t(key))
            .unwrap_or_else(|| key.to_string())
    };
    let doctype = RwSignal::new(String::new());
    let matrix = RwSignal::new(None::<PermissionMatrix>);
    let loading = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    // The cell currently being written, so only that checkbox is disabled while
    // the server commits and rebuilds the policy.
    let busy = RwSignal::new(None::<(String, String)>);
    // The doctype picker lists the ones the Desk actually exposes.
    let doctypes = crate::views::views_components::use_desk_doctypes();

    let load = move || {
        let requested = doctype.get().trim().to_string();
        if requested.is_empty() {
            error.set(Some(tr("views.permissions.choose_doctype")));
            return;
        }
        loading.set(true);
        error.set(None);
        api::drive(async move {
            let result = match DocStore::runtime() {
                Ok(store) => store.permission_matrix(&requested).await,
                Err(message) => Err(message),
            };
            loading.set(false);
            match result {
                Ok(matrix_value) => matrix.set(Some(matrix_value)),
                Err(message) => {
                    matrix.set(None);
                    error.set(Some(message));
                }
            }
        });
    };

    let toggle = Callback::new(move |(role, perm_type, granted): (String, String, bool)| {
        let requested = doctype.get_untracked();
        let key = (role.clone(), perm_type.clone());
        busy.set(Some(key));
        error.set(None);
        api::drive(async move {
            let result = match DocStore::runtime() {
                Ok(store) => {
                    store
                        .set_custom_permission(&requested, &role, &perm_type, granted)
                        .await
                }
                Err(message) => Err(message),
            };
            busy.set(None);
            match result {
                Ok(()) => {
                    matrix.update(|current| {
                        if let Some(current) = current
                            && let Some(grant) = current
                                .grants
                                .iter_mut()
                                .find(|grant| grant.role == role && grant.perm_type == perm_type)
                        {
                            grant.granted = granted;
                        }
                    });
                }
                Err(message) => error.set(Some(message)),
            }
        });
    });

    view! {
        <Card>
            <CardHeader>
                <CardTitle>{tr("views.permissions.title")}</CardTitle>
                <CardDescription>
                    {tr("views.permissions.description")}
                </CardDescription>
            </CardHeader>
            <CardContent class="flex flex-col gap-3">
                <div class="flex flex-wrap items-end gap-2">
                    <div class="w-full min-w-0 sm:w-auto">
                        <SelectNative
                            id="permission-doctype"
                            class="h-8 w-full sm:w-64"
                            aria_label="DocType"
                            value=doctype.read_only()
                            on_change=Callback::new(move |ev: leptos::ev::Event| {
                                doctype.set(event_target_value(&ev));
                                // The loaded matrix belongs to the previous
                                // doctype: leaving it on screen would make its
                                // checkboxes write the old grants to the new
                                // doctype. Clear it until "Cargar" reads the
                                // right one.
                                matrix.set(None);
                                error.set(None);
                            })
                        >
                            <option value="">{tr("views.common.choose_doctype")}</option>
                            {move || {
                                doctypes
                                    .get()
                                    .into_iter()
                                    .map(|name| {
                                        let label = doctype_display(&name);
                                        view! { <option value=name>{label}</option> }
                                    })
                                    .collect_view()
                            }}
                        </SelectNative>
                    </div>
                    <Button
                        variant=ButtonVariant::Default
                        size=ButtonSize::Sm
                        class="w-full sm:w-auto"
                        attr:disabled=loading
                        on:click=move |_| load()
                    >
                        {tr("views.permissions.load")}
                    </Button>
                </div>

                <Show when=move || loading.get()>
                    <p class="text-sm text-muted-foreground">{tr("views.permissions.loading")}</p>
                </Show>
                <Feedback
                    kind=FeedbackKind::Error
                    message=Signal::derive(move || error.get())
                />

                <Show when=move || matrix.get().is_some()>
                    {move || {
                        matrix
                            .get()
                            .map_or_else(
                                || AnyView::from(Fragment::new(Vec::new())),
                                |current| {
                                    matrix_view(current, matrix, busy, toggle, i18n)
                                },
                            )
                    }}
                </Show>
            </CardContent>
        </Card>
    }
}

/// Render one loaded matrix.
///
/// Kept outside the component so every closure returns an owned value: the row
/// and cell iterators move clones of the labels into their views, which is what
/// keeps the nested `view!` macros `'static`.
fn matrix_view(
    current: PermissionMatrix,
    matrix: RwSignal<Option<PermissionMatrix>>,
    busy: RwSignal<Option<(String, String)>>,
    toggle: Callback<(String, String, bool)>,
    i18n: crate::i18n::I18n,
) -> AnyView {
    if current.perm_types.is_empty() {
        return view! {
            <p class="text-sm text-muted-foreground">
                {i18n.t("views.permissions.no_types")}
            </p>
        }
        .into_any();
    }
    let roles = current.roles;
    let headers = roles
        .clone()
        .into_iter()
        .map(|role| view! { <t::TableHead>{role}</t::TableHead> })
        .collect_view();
    let rows = current
        .perm_types
        .into_iter()
        .map(|perm_type| {
            let title = perm_type.clone();
            let cells = roles
                .clone()
                .into_iter()
                .map(|role| {
                    let aria = i18n.t_with(
                        "views.permissions.grant_aria",
                        &[("permission", &perm_type), ("role", &role)],
                    );
                    let checked_role = role.clone();
                    let checked_type = perm_type.clone();
                    let change_role = role;
                    let change_type = perm_type.clone();
                    view! {
                        <t::TableCell class="text-center">
                            <Checkbox
                                // A touch-sized box on a phone; the dense
                                // desktop matrix keeps its original size.
                                class="size-5 sm:size-4"
                                aria_label=aria
                                checked=Signal::derive(move || {
                                    matrix
                                        .get()
                                        .and_then(|current| {
                                            current
                                                .grants
                                                .iter()
                                                .find(|grant| {
                                                    grant.role == checked_role
                                                        && grant.perm_type == checked_type
                                                })
                                                .map(|grant| grant.granted)
                                        })
                                        .unwrap_or(false)
                                })
                                disabled=Signal::derive(move || {
                                    // One write at a time: each one rebuilds the
                                    // installed policy on the server, and two
                                    // concurrent rebuilds would interleave.
                                    busy.get().is_some()
                                })
                                on_checked_change=Callback::new(move |granted: bool| {
                                    toggle.run((
                                        change_role.clone(),
                                        change_type.clone(),
                                        granted,
                                    ));
                                })
                            />
                        </t::TableCell>
                    }
                })
                .collect_view();
            view! {
                <t::TableRow>
                    <t::TableCell class="font-medium">{title}</t::TableCell>
                    {cells}
                </t::TableRow>
            }
        })
        .collect_view();
    view! {
        <div class="w-full overflow-x-auto">
            <t::Table class="min-w-[480px] [&_th:first-child]:pl-0 [&_td:first-child]:pl-0 [&_th:last-child]:pr-0 [&_td:last-child]:pr-0">
                <t::TableHeader class="bg-muted/40">
                    <t::TableRow>
                        <t::TableHead>{i18n.t("views.permissions.column")}</t::TableHead>
                        {headers}
                    </t::TableRow>
                </t::TableHeader>
                <t::TableBody>{rows}</t::TableBody>
            </t::Table>
        </div>
    }
    .into_any()
}
