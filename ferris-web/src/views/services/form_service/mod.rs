//! Form view business logic and state management.
//!
//! This module contains the reactive [`FormService`] that drives the form view:
//! loading, validation, layout building, the field-edit/layout mode and the
//! state the top bar reads. The pure functions it leans on (the field model,
//! `build_form_layout`, `build_form_fields`, `validate_form`,
//! `build_form_payload`, `duplicate_payload`) live in [`layout`] and are
//! re-exported here, so `crate::views::services::…` keeps resolving every path
//! it used to. The write pipeline lives in [`writes`] and the undo/redo stack
//! for layout edits in [`layout_ops`].

mod layout;
mod layout_history;
mod layout_ops;
mod methods;
mod payload;
mod writes;

pub mod condition;

#[cfg(test)]
mod layout_ops_tests;
#[cfg(test)]
mod tests;

pub use layout::{
    FormField, FormSection, FormSectionItem, build_form_fields, build_form_fields_with,
    build_form_layout, form_layout, validate_form, validate_form_with,
};
pub use layout_ops::{LayoutOp, OpWrite};
use methods::install_fetch_from;
pub use payload::{build_form_payload, duplicate_payload, parse_name_list, strip_virtual_fields};
pub use writes::{WriteOutcome, WriteVerb};

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use serde_json::Value;

use crate::api::hooks::{Generation, use_mutation};
use crate::api::meta::docstatus_rank;
use crate::api::types::Neighbors;
use crate::api::{DocStore, DynMeta};

/// The reserved route segment that opens a DocType's layout editor.
///
/// `/desk/:module/:doctype/layout` is a *third* segment the dispatcher must not
/// read as a document name (see `views::doc_route`). It is spelled here, next
/// to the flag it drives, so the route keyword, the service and the breadcrumb
/// trail cannot drift apart.
pub const LAYOUT_SEGMENT: &str = "layout";

/// A stable fingerprint of what the form currently holds.
///
/// Used to tell "edited" from "pristine": the baseline is the fingerprint taken
/// after a load or a successful save, and the form is *dirty* while the current
/// fingerprint differs. Serialising the same payload the save would send keeps
/// the notion of "changed" identical to the notion of "would write something
/// different" — a field the user typed into and then cleared is not dirty.
fn payload_fingerprint(fields: &[FormField]) -> String {
    serde_json::to_string(&build_form_payload(fields)).unwrap_or_default()
}

/// Show a form error and report it to the server log.
///
/// The Desk has no console an operator can read: an error that only reaches
/// it disappears when the tab closes. `POST /api/log/client` turns it into a
/// server log line under one stable target. The report is best-effort — a log
/// sink must never turn a validation failure into a second failure.
pub(crate) fn set_form_error(error: RwSignal<Option<String>>, message: String) {
    error.set(Some(message.clone()));
    // The log sink is a browser channel: on SSR, reporting would self-HTTP to
    // an address that may not exist yet. The server already has its own log
    // line for the same failure.
    #[cfg(target_arch = "wasm32")]
    crate::api::drive(async move {
        let _ = crate::api::log_client("error", &message, Some("desk form")).await;
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = message;
}

/// Service for managing form view state and operations.
///
/// The three route parameters are signals, not `String`s. They were captured
/// once at construction, so the service was frozen on whichever document it was
/// built with: the router does not rebuild a view when only the params change
///, so `/…/TASK00001` → `/…/TASK00002` left the form
/// pointing at `TASK00001` and the next save overwrote it. Reading them per run
/// is what makes the same route definition follow the same document.
#[derive(Clone)]
pub struct FormService {
    pub doctype: Signal<String>,
    pub module: Signal<String>,
    pub docname: Signal<String>,
    pub is_new: Memo<bool>,

    pub meta: RwSignal<Option<DynMeta>>,
    pub fields: RwSignal<Vec<FormField>>,
    pub title: RwSignal<String>,
    pub docstatus: RwSignal<i64>,
    /// The raw document as the API returned it. The form fields only carry what
    /// is editable; the sidebar (owner, created, last edit, tags) and the
    /// engagement panel need the untouched record.
    pub doc: RwSignal<Value>,
    /// Fingerprint of the fields at the last load/save; see [`payload_fingerprint`].
    pub baseline: RwSignal<String>,
    /// Whether the form holds changes that have not been saved.
    pub dirty: Memo<bool>,

    pub loading: RwSignal<bool>,
    pub saving: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub notice: RwSignal<Option<String>>,
    /// The form's catalogue handle. Services run from effects and handlers, so
    /// they carry the `Copy` handle instead of calling `use_context` there.
    pub i18n: crate::i18n::I18n,

    /// The previous/next docname in list order, for the header's navigation.
    pub neighbors: RwSignal<Neighbors>,
    /// Whether the static form sidebar is shown. Runtime toggle (More → sidebar).
    pub sidebar_visible: RwSignal<bool>,
    /// Whether the form is in field-edit mode. It is the layout route's own
    /// flag (`/desk/:module/:doctype/layout`), kept as a signal because the
    /// field surfaces and the inspector subscribe to it.
    pub editing: RwSignal<bool>,
    /// Whether the route is the DocType's **layout** endpoint
    /// (`/desk/:module/:doctype/layout`): the form edits the DocType's shape,
    /// with no document selected and no document writes.
    ///
    /// A memo and not a `bool`, for the same reason the route signals are
    /// signals: the router does not rebuild a view when only the params change
    /// (see the struct docs), so `/…/TASK-0001` → `/…/layout` has to flip this
    /// in place. Derived from `docname`, which is already the route's own
    /// signal — the service never reads route params itself.
    pub layout_mode: Memo<bool>,
    /// The layout edits this client performed, newest last. See [`LayoutOp`].
    ///
    /// Scoped to the layout session: meaningful (and read) only while
    /// `layout_mode` is on. Leaving the editor clears them
    /// ([`FormService::discard_layout_history`]).
    pub undo_stack: RwSignal<Vec<LayoutOp>>,
    /// The undone layout edits, newest last; cleared by a new edit.
    pub redo_stack: RwSignal<Vec<LayoutOp>>,
    /// The write an action started, so its result can be named. See
    /// [`WriteOutcome`].
    pub pending_write: RwSignal<Option<WriteVerb>>,
    /// The result of the last write, waiting to be announced (the top bar turns
    /// it into a transient toast and clears it).
    pub last_write: RwSignal<Option<WriteOutcome>>,
    /// The field whose properties the inspector shows.
    pub selected_field: RwSignal<Option<String>>,

    /// Bumped by [`FormService::reload`] to force the load effect to re-run.
    pub reload: RwSignal<u64>,

    // Derived
    /// Whether the doctype is a Single: one document, edited in place, never
    /// created or deleted from the Desk.
    pub is_single: Memo<bool>,
    /// Whether the doctype participates in the submit lifecycle at all. The
    /// per-transition permissions below are what actually gate each button.
    pub is_submittable: Memo<bool>,
    pub allow_cancel: Memo<bool>,
    pub allow_amend: Memo<bool>,
    pub allow_delete: Memo<bool>,
    pub allow_copy: Memo<bool>,
    pub allow_print: Memo<bool>,
    pub allow_rename: Memo<bool>,

    pub tab_labels: Memo<Vec<(String, String)>>,
    pub default_tab: Memo<String>,
    pub sections: Memo<Vec<FormSection>>,

    // Mutation handlers
    pub save_action: StoredValue<Box<dyn Fn() + Send + Sync>>,
    pub submit_action: StoredValue<Box<dyn Fn() + Send + Sync>>,
    pub cancel_action: StoredValue<Box<dyn Fn() + Send + Sync>>,
    pub amend_action: StoredValue<Box<dyn Fn() + Send + Sync>>,
    pub delete_action: StoredValue<Box<dyn Fn() + Send + Sync>>,
    pub duplicate_action: StoredValue<Box<dyn Fn() + Send + Sync>>,
}

impl FormService {
    /// Create a new form service.
    ///
    /// `doctype`, `module` and `docname` are read again on every load, so the
    /// form follows the URL instead of the document it happened to be built
    /// with.
    pub fn new(doctype: Signal<String>, module: Signal<String>, docname: Signal<String>) -> Self {
        // The layout endpoint is the same route definition as a document
        // (`/desk/:module/:doctype/:name`), so it is recognised from the
        // `docname` signal rather than from a route param the service does not
        // read. It has to be a memo: navigating from a document to `/layout`
        // does not rebuild the view.
        let layout_mode = Memo::new(move |_| docname.get() == LAYOUT_SEGMENT);
        // A layout editor has no record behind it either: treating it as `new`
        // is what keeps the document affordances (navigation, share, tags, the
        // action-bearing fieldtypes) off a form that edits metadata.
        let is_new = Memo::new(move |_| docname.get() == "new" || layout_mode.get());
        let i18n = crate::i18n::use_i18n();

        let meta = RwSignal::new(None::<DynMeta>);
        let fields = RwSignal::new(Vec::<FormField>::new());
        let title = RwSignal::new(String::new());
        let docstatus = RwSignal::new(0i64);
        let doc = RwSignal::new(Value::Null);
        let baseline = RwSignal::new(String::new());
        let loading = RwSignal::new(true);
        let saving = RwSignal::new(false);
        let error = RwSignal::new(None::<String>);
        let notice = RwSignal::new(None::<String>);
        let neighbors = RwSignal::new(Neighbors::default());
        let sidebar_visible = RwSignal::new(true);
        let editing = RwSignal::new(false);
        let selected_field = RwSignal::new(None::<String>);
        let undo_stack = RwSignal::new(Vec::<LayoutOp>::new());
        let redo_stack = RwSignal::new(Vec::<LayoutOp>::new());
        let pending_write = RwSignal::new(None::<WriteVerb>);
        let last_write = RwSignal::new(None::<WriteOutcome>);
        let reload = RwSignal::new(0u64);

        // The layout stacks belong to the **layout session**: undo/redo exists
        // while `/layout` is open and nowhere else. They are seeded from the
        // session store when the editor mounts (the router rebuilds the view
        // when the route changes, and a remount inside the editor must not lose
        // the session), and dropped as soon as the route stops being `/layout`:
        // the document view has no layout on screen, so an undo there would
        // revert a change the user can no longer see — which is exactly what the
        // document view used to offer.
        let route_doctype = doctype;
        let route_layout = layout_mode;
        Effect::new(move |_| {
            let dt = route_doctype.get();
            if dt.is_empty() {
                return;
            }
            if route_layout.get() {
                let stored = layout_history::load(&dt);
                undo_stack.set(stored.undo);
                redo_stack.set(stored.redo);
            } else {
                layout_history::clear(&dt);
                undo_stack.set(Vec::new());
                redo_stack.set(Vec::new());
            }
        });

        // Load metadata and document. The parameter reads below are what this
        // effect subscribes to, so a navigation between two documents that share
        // a route definition re-runs the whole load. Because a second load can
        // be in flight while the first is still running, each run claims a
        // generation and only writes back if it is still the newest one —
        // otherwise a slow answer for the previous document lands on top of the
        // current one and the form shows the wrong fields.
        let generation = Generation::default();
        let navigate = use_navigate();
        // Which `(doctype, docname)` the signals currently describe. A reload
        // of the same form must not flip `loading` back to true: the skeleton
        // unmounted the whole editor on every property save, and the rebuild
        // that followed threw away the inspector's draft state. A route change
        // still gets the skeleton and a clean slate.
        let loaded_for = RwSignal::new((String::new(), String::new()));
        Effect::new(move |_| {
            let dt = doctype.get();
            let name = docname.get();
            let mode_new = name == "new";
            let layout = layout_mode.get();
            // Subscribed so `reload()` (More → Reload) forces a fresh read.
            let _ = reload.get();
            if dt.is_empty() {
                loading.set(false);
                return;
            }

            let route_changed =
                loaded_for.with_untracked(|loaded| loaded.0 != dt || loaded.1 != name);
            if route_changed {
                loading.set(true);
                error.set(None);
                fields.set(Vec::new());
                baseline.set(String::new());
            }

            let mine = generation.claim();
            let guard = generation.clone();
            error.set(None);
            spawn_local(async move {
                let store = match DocStore::runtime() {
                    Ok(s) => s,
                    Err(e) => {
                        if guard.is_current(mine) {
                            set_form_error(error, e);
                            loading.set(false);
                        }
                        return;
                    }
                };
                let m = match store.meta(&dt).await {
                    Ok(Some(m)) => m,
                    Ok(None) => {
                        if guard.is_current(mine) {
                            error.set(Some(i18n.t_with(
                                "components.form.doctype_unregistered",
                                &[("doctype", &dt)],
                            )));
                            loading.set(false);
                        }
                        return;
                    }
                    Err(e) => {
                        if guard.is_current(mine) {
                            set_form_error(error, e);
                            loading.set(false);
                        }
                        return;
                    }
                };
                let doc_value = if layout || mode_new {
                    // The layout editor edits the DocType's *shape*, not a
                    // record, and `layout` is not a document name: reading it
                    // would 404 and paint the load failure over the editor.
                    // Nil document + `mode_new = false` below builds every
                    // declared field with its defaults and no synthetic id
                    // field, which is exactly the empty canvas the editor
                    // wants.
                    Value::Null
                } else if m.is_single {
                    // A Single has one document named after the doctype; read it
                    // through the single endpoint so the framework materialises
                    // it with its defaults on first access.
                    match store.get_single(&dt).await {
                        Ok(d) => d,
                        Err(e) => {
                            if guard.is_current(mine) {
                                set_form_error(error, e);
                                loading.set(false);
                            }
                            return;
                        }
                    }
                } else {
                    // A failed read is a failure, not an empty document.
                    // Swallowing it rendered a form over a document that does
                    // not exist, with an enabled "Guardar" that would have
                    // created one.
                    match store.get(&dt, &name).await {
                        Ok(d) => d,
                        Err(e) => {
                            if guard.is_current(mine) {
                                set_form_error(error, e);
                                loading.set(false);
                            }
                            return;
                        }
                    }
                };
                if !guard.is_current(mine) {
                    return;
                }

                let fields_out = build_form_fields_with(&m, &doc_value, mode_new, &i18n);
                title.set(
                    doc_value
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(dt.as_str())
                        .to_string(),
                );
                docstatus.set(doc_value.get("docstatus").map(docstatus_rank).unwrap_or(0));
                baseline.set(payload_fingerprint(&fields_out));
                fields.set(fields_out);
                doc.set(doc_value);
                let is_single_meta = m.is_single;
                meta.set(Some(m));
                loaded_for.set((dt.clone(), name.clone()));
                loading.set(false);

                // Previous/next in list order, for the header's navigation. Only
                // meaningful for a stored, non-Single document; a new form has no
                // place in the list yet.
                let want_neighbors = !layout && !mode_new && !is_single_meta;
                if want_neighbors {
                    if let Ok(resolved) = store.neighbors(&dt, &name).await
                        && guard.is_current(mine)
                    {
                        neighbors.set(resolved);
                    }
                } else {
                    neighbors.set(Neighbors::default());
                }
            });
        });

        // Derived signals
        // A Single has exactly one document: the form edits it in place and
        // never creates, deletes or navigates.
        let is_single =
            Memo::new(move |_| meta.get().as_ref().map(|m| m.is_single).unwrap_or(false));
        let is_submittable =
            Memo::new(move |_| meta.get().as_ref().map(|m| m.allow_submit).unwrap_or(false));
        let allow_cancel =
            Memo::new(move |_| meta.get().as_ref().map(|m| m.allow_cancel).unwrap_or(false));
        let allow_amend =
            Memo::new(move |_| meta.get().as_ref().map(|m| m.allow_amend).unwrap_or(false));
        let allow_delete =
            Memo::new(move |_| meta.get().as_ref().map(|m| m.allow_delete).unwrap_or(false));
        let allow_copy =
            Memo::new(move |_| meta.get().as_ref().map(|m| m.allow_copy).unwrap_or(false));
        let allow_print =
            Memo::new(move |_| meta.get().as_ref().map(|m| m.allow_print).unwrap_or(false));
        let allow_rename =
            Memo::new(move |_| meta.get().as_ref().map(|m| m.allow_rename).unwrap_or(false));

        // Whether the form holds unsaved edits. `build_form_payload` reads every
        // field signal, so this re-runs on each keystroke.
        let dirty = Memo::new(move |_| {
            let current = payload_fingerprint(&fields.get());
            current != baseline.get()
        });

        let tab_labels = Memo::new(move |_| {
            let fields = fields;
            form_layout(&fields.get()).0
        });

        let default_tab = Memo::new(move |_| {
            let tab_labels = tab_labels;
            tab_labels
                .get()
                .first()
                .map(|(v, _)| v.clone())
                .unwrap_or_default()
        });

        let sections = Memo::new(move |_| {
            let fields = fields;
            form_layout(&fields.get()).2
        });

        // `fetch_from` autocompletion. See [`methods::install_fetch_from`].
        install_fetch_from(fields);

        // Mutation hook. `mutate` returns as soon as the request has been
        // spawned, so both the in-flight state and the failure have to be read
        // from `mutating`/`mutate_error`. Discarding them with `_` (as this
        // used to) is why a rejected save looked exactly like a successful one,
        // and why `saving` had to be cleared right after the call to pretend the
        // round trip had ended: the button stayed enabled while the request was
        // in the air, so two clicks were two `Update`s of the same document
        //.
        let (mutate, mutating, mutate_error, mutate_result) = use_mutation();

        let service = Self {
            doctype,
            module,
            docname,
            is_new,
            layout_mode,
            meta,
            fields,
            title,
            docstatus,
            doc,
            baseline,
            dirty,
            loading,
            saving,
            error,
            notice,
            i18n,
            neighbors,
            sidebar_visible,
            editing,
            selected_field,
            undo_stack,
            redo_stack,
            pending_write,
            last_write,
            reload,
            is_single,
            is_submittable,
            allow_cancel,
            allow_amend,
            allow_delete,
            allow_copy,
            allow_print,
            allow_rename,
            tab_labels,
            default_tab,
            sections,
            // Filled in by `install_writes` right below: an action has to
            // address the signals of the service it belongs to, and those only
            // exist once the struct is built.
            save_action: uninstalled(),
            submit_action: uninstalled(),
            cancel_action: uninstalled(),
            amend_action: uninstalled(),
            delete_action: uninstalled(),
            duplicate_action: uninstalled(),
        };

        // The write pipeline: the lifecycle actions, the mutation hook's
        // outcome and the fold-back of the stored document. It lives in
        // [`writes`] because `mod.rs` is at the file-size budget and because
        // those three pieces are one concern — an action announces the verb,
        // the hook reports in flight, and a single effect decides what
        // finished.
        service.install_writes(mutate, mutating, mutate_error, mutate_result, navigate);

        // The layout route *is* the field-edit mode: the in-form inspector is
        // how a layout is changed, so entering `/layout` turns it on and leaving
        // it turns it off. There is no in-place "Personalizar" any more: the
        // mode has one URL, and the menu navigates to it.
        Effect::new(move |_| {
            if layout_mode.get() {
                editing.set(true);
                selected_field.set(None);
                // A refusal or a failed read from the previous mode must not
                // stay on screen over an editor the user just opened.
                error.set(None);
            } else {
                editing.set(false);
                selected_field.set(None);
            }
        });

        service
    }
}

/// The placeholder an action slot holds before
/// [`FormService::install_writes`] fills it.
///
/// The slot type is a boxed `Fn`, so it needs *a* closure; this one does
/// nothing on purpose. `new` is the only constructor and it installs the real
/// actions before returning, so no button ever reads this value.
fn uninstalled() -> StoredValue<Box<dyn Fn() + Send + Sync>> {
    StoredValue::new(Box::new(|| {}) as Box<dyn Fn() + Send + Sync>)
}
