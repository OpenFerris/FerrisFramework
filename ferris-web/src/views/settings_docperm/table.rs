//! The permission matrix grid: one row per role, one column per right.

use leptos::prelude::*;

use crate::api::authz::{DocPermRow, Right};
use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::table as t;
use crate::i18n::I18n;
use crate::views::views_components::EmptyState;

use super::MatrixState;
use super::text::{field_level_note, right_label, text};

/// Choose between the empty state and the grid.
///
/// The grid is its own component so `Show`'s children stay `Fn`: passing an
/// already-built header row would move it and turn the closure into `FnOnce`.
#[component]
pub(super) fn DocPermTable(
    matrix: RwSignal<Option<MatrixState>>,
    saving: RwSignal<bool>,
    i18n: I18n,
) -> impl IntoView {
    let no_roles = Signal::derive(move || matrix.get().is_some_and(|state| state.roles.is_empty()));
    view! {
        <Show when=move || no_roles.get()>
            <EmptyState
                icon="Users"
                title=text(i18n, "views.docperm.no_roles_title", "No roles to configure")
                description=text(
                    i18n,
                    "views.docperm.no_roles_description",
                    "The server returned no roles for this DocType.",
                )
            />
        </Show>

        <Show when=move || !no_roles.get()>
            <RoleTable matrix=matrix saving=saving i18n=i18n />
        </Show>
    }
}

/// The actual table. Built only when the role list is non-empty.
#[component]
fn RoleTable(
    matrix: RwSignal<Option<MatrixState>>,
    saving: RwSignal<bool>,
    i18n: I18n,
) -> impl IntoView {
    let headers = Right::ALL
        .into_iter()
        .map(|right| {
            let label = right_label(i18n, right);
            view! { <t::TableHead class="text-center">{label}</t::TableHead> }
        })
        .collect_view();
    let if_owner_label = text(i18n, "views.docperm.if_owner", "Owner only");
    let header_label = if_owner_label.clone();
    let if_owner_hint = text(
        i18n,
        "views.docperm.if_owner_hint",
        "The rule only applies to documents the user owns.",
    );

    view! {
        <div class="w-full overflow-x-auto">
            <t::Table class="min-w-[980px] [&_th:first-child]:pl-0 [&_td:first-child]:pl-0 [&_th:last-child]:pr-0 [&_td:last-child]:pr-0">
                <t::TableHeader class="bg-muted/40">
                    <t::TableRow>
                        <t::TableHead>
                            {text(i18n, "views.docperm.role", "Role")}
                        </t::TableHead>
                        {headers}
                        <t::TableHead class="text-center">
                            <span title=if_owner_hint>{header_label}</span>
                        </t::TableHead>
                    </t::TableRow>
                </t::TableHeader>
                <t::TableBody>
                    <For
                        each=move || {
                            matrix
                                .get()
                                .map(|state| state.roles)
                                .unwrap_or_default()
                        }
                        key=|role| role.clone()
                        let:role
                    >
                        <DocPermRowView
                            role=role
                            matrix=matrix
                            saving=saving
                            i18n=i18n
                            if_owner_label=if_owner_label.clone()
                        />
                    </For>
                </t::TableBody>
            </t::Table>
        </div>
        <p class="text-xs text-muted-foreground">
            {move || {
                matrix
                    .get()
                    .map(|state| field_level_note(i18n, state.field_level_rows))
                    .unwrap_or_default()
            }}
        </p>
    }
}

/// One role and its 13 rights, each an independently reactive checkbox.
#[component]
fn DocPermRowView(
    role: String,
    matrix: RwSignal<Option<MatrixState>>,
    saving: RwSignal<bool>,
    i18n: I18n,
    if_owner_label: String,
) -> impl IntoView {
    // Cloning outside the map keeps the iterator's closure `FnMut`: the view
    // macro moves the value it is given, and this row has 14 cells.
    let cell_role = role.clone();
    let cells = Right::ALL
        .into_iter()
        .map(move |right| {
            let permission = right_label(i18n, right);
            let aria = i18n.t_with(
                "views.permissions.grant_aria",
                &[("permission", &permission), ("role", &cell_role)],
            );
            let role = cell_role.clone();
            view! {
                <t::TableCell class="text-center">
                    <RightCheckbox
                        matrix=matrix
                        saving=saving
                        role=role
                        right=right
                        aria_label=aria
                    />
                </t::TableCell>
            }
        })
        .collect_view();

    let owner_aria = i18n.t_with(
        "views.permissions.grant_aria",
        &[("permission", &if_owner_label), ("role", &role)],
    );

    view! {
        <t::TableRow>
            <th
                scope="row"
                class="ferris-table__cell px-3 py-2 text-left align-middle font-medium whitespace-nowrap"
            >
                {role.clone()}
            </th>
            {cells}
            <t::TableCell class="text-center">
                <OwnerCheckbox
                    matrix=matrix
                    saving=saving
                    role=role
                    aria_label=owner_aria
                />
            </t::TableCell>
        </t::TableRow>
    }
}

#[component]
fn RightCheckbox(
    matrix: RwSignal<Option<MatrixState>>,
    saving: RwSignal<bool>,
    role: String,
    right: Right,
    aria_label: String,
) -> impl IntoView {
    let checked_role = role.clone();
    let change_role = role;
    let checked = Signal::derive(move || {
        matrix
            .get()
            .and_then(|state| state.draft.get(&checked_role).map(|row| right.get(row)))
            .unwrap_or(false)
    });
    let on_change = Callback::new(move |value: bool| {
        let role = change_role.clone();
        matrix.update(|current| {
            if let Some(state) = current {
                let doctype = state.doctype.clone();
                let row = state
                    .draft
                    .entry(role.clone())
                    .or_insert_with(|| DocPermRow::blank(&doctype, &role));
                right.set(row, value);
            }
        });
    });

    view! {
        <Checkbox
            class="size-5 sm:size-4"
            aria_label=aria_label
            checked=checked
            disabled=Signal::derive(move || saving.get())
            on_checked_change=on_change
        />
    }
}

#[component]
fn OwnerCheckbox(
    matrix: RwSignal<Option<MatrixState>>,
    saving: RwSignal<bool>,
    role: String,
    aria_label: String,
) -> impl IntoView {
    let checked_role = role.clone();
    let change_role = role;
    let checked = Signal::derive(move || {
        matrix
            .get()
            .and_then(|state| state.draft.get(&checked_role).map(|row| row.if_owner))
            .unwrap_or(false)
    });
    let on_change = Callback::new(move |value: bool| {
        let role = change_role.clone();
        matrix.update(|current| {
            if let Some(state) = current {
                let doctype = state.doctype.clone();
                let row = state
                    .draft
                    .entry(role.clone())
                    .or_insert_with(|| DocPermRow::blank(&doctype, &role));
                row.if_owner = value;
            }
        });
    });

    view! {
        <Checkbox
            class="size-5 sm:size-4"
            aria_label=aria_label
            checked=checked
            disabled=Signal::derive(move || saving.get())
            on_checked_change=on_change
        />
    }
}
