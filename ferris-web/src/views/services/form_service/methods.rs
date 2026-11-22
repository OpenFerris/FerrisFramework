//! The thin accessors on [`FormService`], plus the one reactive wiring helper
//! that used to live inline in the constructor.
//!
//! They are split out of `mod.rs` so that file holds the struct and the
//! constructor and stays under the 600-line budget. Rust allows one type's
//! `impl` to be fragmented across files, so this is behaviour- and
//! visibility-preserving: same methods, same module path
//! (`crate::views::services::form_service::FormService`).
//!
//! Every write accessor goes through [`FormService::refuse_in_layout_mode`].
//! The top bar hides the buttons in that mode, and the service refuses anyway:
//! a layout editor that can be talked into an `Update` of a document named
//! `layout` would create a record from a page that has none.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use serde_json::Value;

use crate::api::DocStore;
use crate::api::meta::docstatus_rank;

use super::{FormField, FormService, payload_fingerprint};

/// What the user reads when a write reaches the service in layout mode.
const LAYOUT_MODE_REFUSAL: &str = "El modo diseño edita la estructura de la doctype, no documentos: guardar, enviar, cancelar \
     y eliminar están desactivados.";

impl FormService {
    /// Refuse a document write while the layout editor is open.
    ///
    /// Returns `true` when the caller must stop. The message goes to `error`,
    /// not `notice`: the view paints `notice` as a *success* feedback, and a
    /// refused write is not a success. It cannot be mistaken for a load failure
    /// either — that branch needs the fields to be empty, and the layout editor
    /// always has them.
    fn refuse_in_layout_mode(&self) -> bool {
        if !self.layout_mode.get_untracked() {
            return false;
        }
        self.error.set(Some(LAYOUT_MODE_REFUSAL.to_string()));
        true
    }

    /// Execute the save action.
    pub fn save(&self) {
        if self.refuse_in_layout_mode() {
            return;
        }
        self.save_action.with_value(|f| f());
    }

    /// Execute the submit action.
    pub fn submit(&self) {
        if self.refuse_in_layout_mode() {
            return;
        }
        self.submit_action.with_value(|f| f());
    }

    /// Execute the cancel action.
    pub fn cancel(&self) {
        if self.refuse_in_layout_mode() {
            return;
        }
        self.cancel_action.with_value(|f| f());
    }

    /// Execute the amend action.
    pub fn amend(&self) {
        if self.refuse_in_layout_mode() {
            return;
        }
        self.amend_action.with_value(|f| f());
    }

    /// Execute the delete action.
    pub fn delete(&self) {
        if self.refuse_in_layout_mode() {
            return;
        }
        self.delete_action.with_value(|f| f());
    }

    /// Execute the duplicate action.
    pub fn duplicate(&self) {
        if self.refuse_in_layout_mode() {
            return;
        }
        self.duplicate_action.with_value(|f| f());
    }

    /// Re-read the document and its metadata from the source of truth.
    pub fn reload(&self) {
        self.reload.update(|n| *n += 1);
    }

    /// Fold a server-returned document into the live form **without a remount**.
    ///
    /// Used by out-of-band writes the server answers with the stored document
    /// (the sidebar's tags). Replacing `doc` keeps the provenance, the status
    /// badge and every derived memo current; the editable values are updated in
    /// place by [`FormService::set_field_value`], so the form keeps its focus,
    /// its active tab and any unsaved edit in another field. A full reload here
    /// used to drop the whole form behind a skeleton and re-fetch a dozen
    /// endpoints for a single tag click.
    pub fn apply_stored_doc(&self, stored: &Value) {
        if !stored.is_object() {
            return;
        }
        self.doc.set(stored.clone());
        self.docstatus
            .set(stored.get("docstatus").map(docstatus_rank).unwrap_or(0));
    }

    /// Write one field's value on the **existing** signal.
    ///
    /// The form renderer snapshots the field list, so replacing a `FormField`
    /// would not be seen; mutating its `RwSignal` is the supported in-place path
    /// (the same one `fetch_from` and the child table use). A field that is not
    /// in the form (hidden, or filtered out) is a no-op.
    pub fn set_field_value(&self, fieldname: &str, value: &str) {
        self.fields.with_untracked(|fields| {
            if let Some(field) = fields.iter().find(|field| field.fieldname == fieldname)
                && field.value.get_untracked() != value
            {
                field.value.set(value.to_string());
            }
        });
    }

    /// Mark the current field values as the saved state.
    ///
    /// Only for a write that persisted **exactly what the form holds**: calling
    /// it while another field is dirty would mark that edit as saved.
    pub fn accept_current_as_saved(&self) {
        self.baseline.set(
            self.fields
                .with_untracked(|fields| payload_fingerprint(fields)),
        );
    }

    /// This document's route, for links from the sidebar and menus.
    pub fn route(&self, name: &str) -> String {
        format!(
            "/desk/{}/{}/{}",
            crate::desk::session::path_segment(&self.module.get()),
            crate::desk::session::path_segment(&self.doctype.get()),
            crate::desk::session::path_segment(name)
        )
    }

    /// Navigate back to list view.
    pub fn navigate_back(&self) {
        let url = format!(
            "/desk/{}/{}/",
            crate::desk::session::path_segment(&self.module.get()),
            crate::desk::session::path_segment(&self.doctype.get())
        );
        use_navigate()(&url, Default::default());
    }

    /// Navigate to new document.
    pub fn navigate_new(&self) {
        let url = format!(
            "/desk/{}/{}/new",
            crate::desk::session::path_segment(&self.module.get()),
            crate::desk::session::path_segment(&self.doctype.get())
        );
        use_navigate()(&url, Default::default());
    }
}

/// Wire the `fetch_from` autocompletion effect into the current reactive owner.
///
/// A field that declares `"source.target"` mirrors `target` of the document its
/// `source` `Link` points at, Frappe's `fetch_from`. The effect subscribes to
/// the source signals (and, under `fetch_if_empty`, to the target) so a change
/// re-fetches; reading `fields.get()` subscribes to the *list*, which is
/// replaced on every load, so the wiring follows a navigation.
///
/// A failed or empty read leaves the target untouched rather than blanking a
/// value the user may have typed — the same "never destroy input on a failed
/// fetch" rule the rest of the form follows.
///
/// It lives here, not inline in the constructor, so `mod.rs` stays inside the
/// 600-line budget.
pub(super) fn install_fetch_from(fields: RwSignal<Vec<FormField>>) {
    Effect::new(move |_| {
        let current = fields.get();
        for f in &current {
            let Some(spec) = f.fetch_from.as_deref() else {
                continue;
            };
            let Some((source_field, target_field)) = spec.split_once('.') else {
                continue;
            };
            let Some(source) = current.iter().find(|s| s.fieldname == source_field) else {
                continue;
            };
            let link_value = source.value.get();
            let link_value = link_value.trim().to_string();
            if link_value.is_empty() {
                continue;
            }
            if f.fetch_if_empty && !f.value.get().trim().is_empty() {
                continue;
            }
            let Some(target_doctype) = source.options.clone() else {
                continue;
            };
            let target = f.value;
            let field = target_field.to_string();
            spawn_local(async move {
                let Ok(store) = DocStore::runtime() else {
                    return;
                };
                if let Ok(doc) = store.get(&target_doctype, &link_value).await
                    && let Some(value) = doc.get(&field).and_then(serde_json::Value::as_str)
                {
                    target.set(value.to_string());
                }
            });
        }
    });
}
