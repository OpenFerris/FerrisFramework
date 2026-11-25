//! Settings → Permissions: the `DocPerm` permission matrix.
//!
//! Frappe's Role Permissions Builder writes one `DocPerm` row per
//! `(doctype, role, permlevel)` and the engine installs those rows as its live
//! policy. This screen is that builder's main grid:
//!
//! 1. the administrator filters and picks a `DocType` from the Desk's catalogue;
//! 2. the server's live rows and the role list are read together;
//! 3. every role is one row and every document right one checkbox;
//! 4. **Save changes** upserts the rows that gained a right and deletes the
//!    rows that lost their last one, then re-reads the server.
//!
//! Only permission level 0 is offered. The engine reports `permlevel > 0` rules
//! as untranslated today (they are a field-level concept the policy builder does
//! not consume yet), so editing them here would be a lie. Rows at other levels
//! are counted and disclosed instead of silently rewritten.
//!
//! The custom permission *types* (<https://docs.frappe.io/framework/permission-types>)
//! stay on their own card below this one; this screen does not replace them.

mod table;
mod text;

use std::collections::{BTreeMap, BTreeSet};

use leptos::prelude::*;
use leptos::task::spawn_local;
use table::DocPermTable;
use text::text;

use crate::api::authz::{self, DocPermRow};
use crate::api::types::PermissionMatrix;
use crate::app::doctype_display;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::card::{Card, CardContent, CardDescription, CardHeader, CardTitle};
use crate::components::ui::input::{Input, InputType};
use crate::components::ui::select_native::SelectNative;
use crate::components::ui::toast_custom::_context::ToasterContext;
use crate::i18n::use_i18n;
use crate::views::views_components::{
    EmptyState, Feedback, FeedbackKind, LoadingLines, use_desk_doctypes,
};

/// The loaded matrix plus the draft the checkboxes write to.
///
/// `saved` is the server state and `draft` the user's edits. Keeping both means
/// "dirty" and "delete instead of upsert" are decided from the same snapshot,
/// without a second round trip.
#[derive(Clone, Default, PartialEq, Eq)]
struct MatrixState {
    doctype: String,
    roles: Vec<String>,
    draft: BTreeMap<String, DocPermRow>,
    saved: BTreeMap<String, DocPermRow>,
    /// Rows at `permlevel > 0`, which this screen does not edit.
    field_level_rows: usize,
    /// Roles that own at least one of those hidden rows. `DELETE` removes every
    /// level for a role, so these roles must never take that path.
    field_level_roles: BTreeSet<String>,
}

impl MatrixState {
    /// Read both endpoints and build the editable state.
    async fn load(doctype: &str) -> Result<Self, String> {
        let matrix = authz::permission_matrix(doctype).await?;
        let rows = authz::list_role_permissions(doctype).await?;
        Ok(Self::assemble(doctype, matrix, rows))
    }

    /// Combine the role list with the stored rows.
    ///
    /// Rows whose role the matrix endpoint did not list are kept as rows
    /// anyway: dropping them would hide a live rule, and saving any other cell
    /// would then leave it untouched but invisible.
    fn assemble(doctype: &str, matrix: PermissionMatrix, rows: Vec<DocPermRow>) -> Self {
        let mut roles = matrix.roles;
        let mut saved = BTreeMap::new();
        let mut field_level_rows = 0;
        let mut field_level_roles = BTreeSet::new();
        for row in rows {
            if !row.doctype.is_empty() && row.doctype != doctype {
                continue;
            }
            if row.permlevel == 0 {
                roles.push(row.role.clone());
                saved.insert(row.role.clone(), row);
            } else {
                field_level_rows += 1;
                field_level_roles.insert(row.role);
            }
        }
        roles.sort();
        roles.dedup();
        let draft = roles
            .iter()
            .map(|role| {
                let row = saved
                    .get(role)
                    .cloned()
                    .unwrap_or_else(|| DocPermRow::blank(doctype, role));
                (role.clone(), row)
            })
            .collect();
        Self {
            doctype: doctype.to_string(),
            roles,
            draft,
            saved,
            field_level_rows,
            field_level_roles,
        }
    }

    fn saved_row(&self, role: &str) -> DocPermRow {
        self.saved
            .get(role)
            .cloned()
            .unwrap_or_else(|| DocPermRow::blank(&self.doctype, role))
    }

    fn draft_row(&self, role: &str) -> DocPermRow {
        self.draft
            .get(role)
            .cloned()
            .unwrap_or_else(|| DocPermRow::blank(&self.doctype, role))
    }

    fn is_dirty(&self) -> bool {
        self.roles
            .iter()
            .any(|role| self.draft_row(role) != self.saved_row(role))
    }

    /// `(role, draft, had_a_saved_row)` for every role the user changed.
    fn changes(&self) -> Vec<(String, DocPermRow, bool)> {
        self.roles
            .iter()
            .filter_map(|role| {
                let draft = self.draft_row(role);
                (draft != self.saved_row(role))
                    .then(|| (role.clone(), draft, self.saved.contains_key(role)))
            })
            .collect()
    }
}

/// One screen, one state machine: the picker, its status and the grid all
/// read the same draft, so splitting the view would only spread the coupling.
#[allow(clippy::too_many_lines)]
#[allow(clippy::must_use_candidate)]
#[component]
pub fn DocPermMatrix() -> impl IntoView {
    let i18n = use_i18n();
    let toaster = use_context::<ToasterContext>();

    let doctypes = use_desk_doctypes();
    let query = RwSignal::new(String::new());
    let doctype = RwSignal::new(String::new());
    let matrix = RwSignal::new(None::<MatrixState>);
    let loading = RwSignal::new(false);
    let saving = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let notice = RwSignal::new(None::<String>);

    // The filter narrows the picker but never drops the current selection, so
    // typing cannot silently change the DocType the loaded matrix belongs to.
    let filtered_doctypes = move || {
        let needle = query.get().trim().to_lowercase();
        let selected = doctype.get();
        let all = doctypes.get();
        let mut shown: Vec<String> = all
            .iter()
            .filter(|name| needle.is_empty() || name.to_lowercase().contains(&needle))
            .cloned()
            .collect();
        if !selected.is_empty() && all.contains(&selected) && !shown.contains(&selected) {
            shown.push(selected);
        }
        shown
    };

    let load = Callback::new(move |()| {
        let requested = doctype.get_untracked().trim().to_string();
        if requested.is_empty() {
            error.set(Some(text(
                i18n,
                "views.permissions.choose_doctype",
                "Choose a DocType from the list.",
            )));
            return;
        }
        loading.set(true);
        error.set(None);
        notice.set(None);
        let requested_for_task = requested;
        spawn_local(async move {
            let result = MatrixState::load(&requested_for_task).await;
            loading.set(false);
            // The picker can move on while the two reads are in flight; a slow
            // answer for the old DocType must not overwrite the new selection.
            if doctype.get_untracked() != requested_for_task {
                return;
            }
            match result {
                Ok(state) => matrix.set(Some(state)),
                Err(message) => {
                    matrix.set(None);
                    error.set(Some(message));
                }
            }
        });
    });

    let save = Callback::new(move |()| {
        let Some(state) = matrix.get_untracked() else {
            return;
        };
        if !state.is_dirty() {
            return;
        }
        let changes = state.changes();
        let hidden_roles = state.field_level_roles.clone();
        let target = state.doctype;
        let saved_message = text(
            i18n,
            "views.docperm.saved",
            "Role permissions saved. The live policy was rebuilt.",
        );
        let toaster = toaster.clone();
        saving.set(true);
        error.set(None);
        notice.set(None);
        spawn_local(async move {
            let mut failure = None;
            for (role, draft, had_row) in changes {
                let result = if draft.has_rights() {
                    authz::upsert_role_permission(&draft).await.map(|_| ())
                } else if had_row {
                    if hidden_roles.contains(&role) {
                        // The delete route drops every level for the role. Keep
                        // the hidden field-level rules and empty the visible
                        // level-0 row instead.
                        authz::upsert_role_permission(&draft).await.map(|_| ())
                    } else {
                        authz::delete_role_permission(&target, &role).await
                    }
                } else {
                    Ok(())
                };
                if let Err(message) = result {
                    failure = Some(message);
                    break;
                }
            }

            // Re-read unconditionally: after a partial write the server is the
            // only honest source of what the screen may show.
            let reloaded = MatrixState::load(&target).await;
            saving.set(false);
            // The picker may have moved on while the writes were in flight; a
            // late answer for the old DocType must not replace the new screen.
            let still_selected = doctype.get_untracked() == target;
            match reloaded {
                Ok(state) => {
                    if still_selected {
                        matrix.set(Some(state));
                    }
                    if let Some(message) = failure {
                        if still_selected {
                            error.set(Some(message.clone()));
                        }
                        if let Some(toaster) = &toaster {
                            toaster.error(message);
                        }
                    } else {
                        if still_selected {
                            notice.set(Some(saved_message.clone()));
                        }
                        if let Some(toaster) = &toaster {
                            toaster.success(saved_message);
                        }
                    }
                }
                Err(message) => {
                    if still_selected {
                        matrix.set(None);
                        error.set(Some(message.clone()));
                    }
                    if let Some(toaster) = &toaster {
                        toaster.error(message);
                    }
                }
            }
        });
    });

    let discard = Callback::new(move |()| {
        matrix.update(|current| {
            if let Some(state) = current {
                let doctype = state.doctype.clone();
                let saved = state.saved.clone();
                let draft: BTreeMap<String, DocPermRow> = state
                    .roles
                    .iter()
                    .map(|role| {
                        let row = saved
                            .get(role)
                            .cloned()
                            .unwrap_or_else(|| DocPermRow::blank(&doctype, role));
                        (role.clone(), row)
                    })
                    .collect();
                state.draft = draft;
            }
        });
        error.set(None);
        notice.set(None);
    });

    let dirty = Signal::derive(move || matrix.get().is_some_and(|state| state.is_dirty()));
    let has_matrix = Signal::derive(move || matrix.get().is_some());
    let idle =
        Signal::derive(move || !loading.get() && matrix.get().is_none() && error.get().is_none());

    view! {
        <Card>
            <CardHeader>
                <CardTitle>
                    {text(i18n, "views.docperm.title", "Role permissions")}
                </CardTitle>
                <CardDescription>
                    {text(
                        i18n,
                        "views.docperm.description",
                        "Choose which actions each role may perform on one DocType. Saving rebuilds the live policy immediately. Requires System Manager or Administrator.",
                    )}
                </CardDescription>
            </CardHeader>
            <CardContent class="flex flex-col gap-4">
                <div class="flex flex-col gap-3 lg:flex-row lg:flex-wrap lg:items-end">
                    <div class="flex w-full min-w-0 flex-col gap-1 lg:w-60">
                        <label for="docperm-filter" class="text-sm font-medium">
                            {text(i18n, "views.docperm.filter_label", "Filter DocTypes")}
                        </label>
                        <Input
                            id="docperm-filter"
                            r#type=InputType::Search
                            class="h-8"
                            bind_value=query
                            aria_label=text(
                                i18n,
                                "views.docperm.filter_label",
                                "Filter DocTypes",
                            )
                            placeholder=text(
                                i18n,
                                "views.docperm.filter_placeholder",
                                "Type to narrow the list…",
                            )
                        />
                    </div>
                    <div class="flex w-full min-w-0 flex-col gap-1 lg:w-60">
                        <label for="docperm-doctype" class="text-sm font-medium">
                            {text(i18n, "views.docperm.doctype_label", "DocType")}
                        </label>
                        <SelectNative
                            id="docperm-doctype"
                            class="h-8"
                            aria_label=text(i18n, "views.docperm.doctype_label", "DocType")
                            value=doctype.read_only()
                            on_change=Callback::new(move |ev: leptos::ev::Event| {
                                doctype.set(event_target_value(&ev));
                                // The matrix belongs to the previous DocType;
                                // keeping it on screen would let its checkboxes
                                // write rows for the wrong DocType.
                                matrix.set(None);
                                error.set(None);
                                notice.set(None);
                            })
                        >
                            <option value="">
                                {text(
                                    i18n,
                                    "views.common.choose_doctype",
                                    "Choose a DocType…",
                                )}
                            </option>
                            {move || {
                                filtered_doctypes()
                                    .into_iter()
                                    .map(|name| {
                                        let label = doctype_display(&name);
                                        view! { <option value=name>{label}</option> }
                                    })
                                    .collect_view()
                            }}
                        </SelectNative>
                    </div>
                    <div class="flex w-full flex-wrap items-center gap-2 lg:w-auto">
                        <Button
                            variant=ButtonVariant::Default
                            size=ButtonSize::Sm
                            attr:disabled=move || loading.get() || saving.get()
                            on:click=move |_| load.run(())
                        >
                            {text(i18n, "views.permissions.load", "Load")}
                        </Button>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            attr:disabled=move || {
                                loading.get() || saving.get() || !dirty.get()
                            }
                            on:click=move |_| save.run(())
                        >
                            {move || {
                                if saving.get() {
                                    text(i18n, "views.docperm.saving", "Saving…")
                                } else {
                                    text(i18n, "views.docperm.save", "Save changes")
                                }
                            }}
                        </Button>
                        <Button
                            variant=ButtonVariant::Ghost
                            size=ButtonSize::Sm
                            attr:disabled=move || {
                                loading.get() || saving.get() || !dirty.get()
                            }
                            on:click=move |_| discard.run(())
                        >
                            {text(i18n, "views.docperm.discard", "Discard changes")}
                        </Button>
                    </div>
                </div>

                <Show when=move || dirty.get()>
                    <p class="text-sm text-warning" role="status">
                        {text(
                            i18n,
                            "views.docperm.unsaved",
                            "Unsaved changes.",
                        )}
                    </p>
                </Show>

                <Show when=move || loading.get()>
                    <LoadingLines lines=5 />
                </Show>

                <Feedback
                    kind=FeedbackKind::Error
                    message=Signal::derive(move || error.get())
                />
                <Feedback
                    kind=FeedbackKind::Success
                    message=Signal::derive(move || notice.get())
                />

                <Show when=move || idle.get()>
                    <EmptyState
                        icon="Lock"
                        title=text(i18n, "views.docperm.empty_title", "Choose a DocType")
                        description=text(
                            i18n,
                            "views.docperm.empty_description",
                            "Filter the list, pick a DocType and press Load to edit its role permissions.",
                        )
                    />
                </Show>

                <Show when=move || has_matrix.get()>
                    <DocPermTable matrix=matrix saving=saving i18n=i18n />
                </Show>
            </CardContent>
        </Card>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::authz::Right;

    fn matrix(roles: &[&str]) -> PermissionMatrix {
        PermissionMatrix {
            doctype: "Task".to_string(),
            roles: roles.iter().map(|role| (*role).to_string()).collect(),
            ..PermissionMatrix::default()
        }
    }

    #[test]
    fn assemble_unions_matrix_roles_with_stored_rows() {
        let mut stored = DocPermRow::blank("Task", "Legacy");
        stored.read = true;
        let state = MatrixState::assemble(
            "Task",
            matrix(&["Approver", "System Manager"]),
            vec![stored],
        );
        assert_eq!(state.roles, vec!["Approver", "Legacy", "System Manager"]);
        assert!(state.draft_row("Legacy").read);
        assert!(state.saved.contains_key("Legacy"));
        assert!(!state.draft_row("Approver").has_rights());
        assert!(!state.is_dirty());
    }

    #[test]
    fn non_zero_permlevels_are_counted_but_not_editable() {
        let mut field_level = DocPermRow::blank("Task", "Auditor");
        field_level.permlevel = 2;
        field_level.read = true;
        let state = MatrixState::assemble("Task", matrix(&["Approver"]), vec![field_level]);
        assert_eq!(state.field_level_rows, 1);
        assert!(!state.roles.contains(&"Auditor".to_string()));
        assert!(!state.saved.contains_key("Auditor"));
    }

    #[test]
    fn a_role_with_hidden_levels_is_never_deleted_wholesale() {
        let mut visible = DocPermRow::blank("Task", "Auditor");
        visible.read = true;
        let mut hidden = DocPermRow::blank("Task", "Auditor");
        hidden.permlevel = 2;
        hidden.read = true;
        let state = MatrixState::assemble("Task", matrix(&["Auditor"]), vec![visible, hidden]);
        assert_eq!(state.field_level_rows, 1);
        assert!(state.field_level_roles.contains("Auditor"));
        assert!(state.saved.contains_key("Auditor"));
    }

    #[test]
    fn toggling_a_right_makes_the_state_dirty() {
        let mut state = MatrixState::assemble("Task", matrix(&["Approver"]), vec![]);
        assert!(!state.is_dirty());
        Right::Read.set(state.draft.get_mut("Approver").unwrap(), true);
        assert!(state.is_dirty());
        let changes = state.changes();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, "Approver");
        assert!(changes[0].1.read);
        assert!(!changes[0].2, "a new row had no saved row to delete");
    }

    #[test]
    fn clearing_every_right_marks_the_row_for_deletion() {
        let mut stored = DocPermRow::blank("Task", "Approver");
        stored.read = true;
        let mut state = MatrixState::assemble("Task", matrix(&["Approver"]), vec![stored]);
        Right::Read.set(state.draft.get_mut("Approver").unwrap(), false);
        let changes = state.changes();
        assert_eq!(changes.len(), 1);
        assert!(!changes[0].1.has_rights());
        assert!(changes[0].2, "the saved row must be deleted");
    }

    #[test]
    fn clearing_a_new_blank_row_is_not_a_change() {
        let state = MatrixState::assemble("Task", matrix(&["Approver"]), vec![]);
        assert!(state.changes().is_empty());
    }
}
