//! The form's write pipeline: the five lifecycle actions, the mutation hook's
//! outcome, and the fold-back of the stored document into the form.
//!
//! It lives outside `mod.rs` (which keeps the struct and the constructor) for
//! two reasons. First, the block is the largest part of the constructor and
//! `mod.rs` is already at the repository's file-size budget. Second — and this
//! is why the split is *this* seam and not an arbitrary one — the three pieces
//! only make sense together: an action announces what it is about to do
//! (`pending_write`), the hook flips `mutating`, and one effect is the only
//! place allowed to decide that the write *finished* and with what result.
//!
//! That last point is what the two outcome signals are for. `use_mutation`
//! exposes "in flight" and "error"; neither can tell a successful submit from a
//! successful delete, and `Delete` answers `Value::Null`, so the fold-back
//! effect skips it. The top bar turns `last_write` into the transient toast
//! (`WriteVerb::done_message` / `error_prefix`), which is why the verb is part
//! of the outcome instead of being guessed at the call site.

use std::sync::Arc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::NavigateOptions;
use serde_json::Value;

use crate::api::DocStore;
use crate::api::hooks::MutationType;
use crate::api::meta::docstatus_rank;

use super::{
    FormService, build_form_payload, duplicate_payload, payload_fingerprint, set_form_error,
    strip_virtual_fields, validate_form,
};

/// The `mutate` handle [`crate::api::hooks::use_mutation`] returns.
///
/// Named here so [`FormService::install_writes`] can take it as one argument
/// instead of spelling the `Arc<dyn Fn …>` twice.
pub(super) type MutationFn = Arc<dyn Fn(String, String, Option<Value>, MutationType) + Send + Sync>;

/// Which write a form action started.
///
/// The verb travels with the outcome because the mutation hook reports one
/// generic error and one generic result; without it the toast could not say
/// "Documento enviado" instead of "Documento guardado". `Undo`/`Redo` are here
/// for the same reason: they write through `DocStore` directly (they have no
/// document to mutate) but must report through the same channel, so the bar has
/// exactly one place to read outcomes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteVerb {
    Save,
    Submit,
    Cancel,
    Amend,
    Delete,
    Duplicate,
    Undo,
    Redo,
    /// A field moved to another position (`insert_after`): a layout edit, not a
    /// document write.
    Move,
    /// The DocType-level settings dialog (`title_field`, `naming_series`, the
    /// per-view properties): a layout edit of the form, not of a document.
    Settings,
}

impl WriteVerb {
    /// What the toast says when the write landed.
    #[must_use]
    pub fn done_message(self) -> &'static str {
        match self {
            WriteVerb::Save => "Documento guardado.",
            WriteVerb::Submit => "Documento enviado.",
            WriteVerb::Cancel => "Documento cancelado.",
            WriteVerb::Amend => "Documento reabierto como borrador.",
            WriteVerb::Delete => "Documento eliminado.",
            WriteVerb::Duplicate => "Documento duplicado.",
            WriteVerb::Undo => "Cambio deshecho.",
            WriteVerb::Redo => "Cambio rehecho.",
            WriteVerb::Move => "Campo movido.",
            WriteVerb::Settings => "Ajustes del DocType guardados.",
        }
    }

    /// How the toast opens when the write failed; the server's message follows.
    #[must_use]
    pub fn error_prefix(self) -> &'static str {
        match self {
            WriteVerb::Save => "No se pudo guardar",
            WriteVerb::Submit => "No se pudo enviar",
            WriteVerb::Cancel => "No se pudo cancelar",
            WriteVerb::Amend => "No se pudo reabrir",
            WriteVerb::Delete => "No se pudo eliminar",
            WriteVerb::Duplicate => "No se pudo duplicar",
            WriteVerb::Undo => "No se pudo deshacer",
            WriteVerb::Redo => "No se pudo rehacer",
            WriteVerb::Move => "No se pudo mover el campo",
            WriteVerb::Settings => "No se pudieron guardar los ajustes del DocType",
        }
    }
}

/// The result of the last write the service started, waiting to be announced.
///
/// The top bar consumes it (and clears it) to raise exactly one toast per
/// write; a failure carries the server's message so the toast can show it
/// verbatim instead of a generic "algo falló".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    Succeeded(WriteVerb),
    Failed(WriteVerb, String),
}

impl FormService {
    /// Install the lifecycle actions and the effects that report their result.
    ///
    /// Called once, right after the service exists, because an action has to
    /// address the *same* signals the form paints (`saving`, `error`, `doc`,
    /// `baseline`); building them next to the struct fields would mean passing
    /// a dozen handles through a free function instead of using `self`.
    ///
    /// The actions are installed into the `StoredValue` slots the struct was
    /// built with (see `mod.rs`): those slots are the only way `methods.rs` can
    /// call an action without the closure types leaking into the public API.
    pub(super) fn install_writes<N>(
        &self,
        mutate: MutationFn,
        mutating: ReadSignal<bool>,
        mutate_error: ReadSignal<Option<String>>,
        mutate_result: ReadSignal<Option<Value>>,
        navigate: N,
    ) where
        N: Fn(&str, NavigateOptions) + Clone + Send + Sync + 'static,
    {
        // Copied out of `self` once: the closures below are `'static`, so they
        // cannot capture the borrow of the service they were installed from.
        let doctype = self.doctype;
        let docname = self.docname;
        let module = self.module;
        let meta = self.meta;
        let fields = self.fields;
        let doc = self.doc;
        let docstatus = self.docstatus;
        let baseline = self.baseline;
        let is_new = self.is_new;
        let is_single = self.is_single;
        let allow_copy = self.allow_copy;
        let loading = self.loading;
        let saving = self.saving;
        let error = self.error;
        let notice = self.notice;
        let reload = self.reload;
        let pending_write = self.pending_write;
        let last_write = self.last_write;

        // Publish the mutation outcome into the signals the view paints.
        // `form_view.rs` renders `error` and `notice`, so routing the error here
        // is all it takes for a failed save, submit, cancel, amend or delete to
        // stop being silent. The error is also cleared when the *next* write
        // starts: `use_mutation` resets its own signal, but this one would
        // otherwise keep the previous failure on screen after a retry that
        // succeeded.
        Effect::new(move |_| {
            let in_flight = mutating.get();
            saving.set(in_flight);
            match mutate_error.get() {
                Some(e) => set_form_error(error, e),
                None if in_flight => error.set(None),
                None => {}
            }
        });

        // Report the outcome of the write *this* service started, once.
        //
        // `seen_in_flight` is what makes "finished" reliable: `mutate` flips
        // `mutating` synchronously, but an effect may also run while a verb is
        // pending and the request has not been spawned yet. Without the flag
        // that first pass would announce a success that never happened. Only a
        // pass that has observed the request in flight may conclude anything.
        let seen_in_flight = RwSignal::new(false);
        Effect::new(move |_| {
            let in_flight = mutating.get();
            let Some(verb) = pending_write.get() else {
                return;
            };
            if in_flight {
                seen_in_flight.set(true);
                return;
            }
            if !seen_in_flight.get_untracked() {
                return;
            }
            seen_in_flight.set(false);
            pending_write.set(None);
            last_write.set(Some(match mutate_error.get() {
                Some(message) => WriteOutcome::Failed(verb, message),
                None => WriteOutcome::Succeeded(verb),
            }));
        });

        // After a successful write, fold the engine's answer back into the form
        // without a second read: the mutating endpoints return the stored
        // document, so the sidebar's edit info, the status badge and the dirty
        // baseline all update from the response. The result is `Null` for a
        // delete, which is skipped (the caller navigates away).
        //
        // `fields.with_untracked` is deliberate: reading the fields reactively
        // here would make the effect re-run on every keystroke and keep rewriting
        // the baseline, so the form could never look dirty.
        let saved_doc = mutate_result;
        Effect::new(move |_| {
            let Some(stored) = saved_doc.get() else {
                return;
            };
            if !stored.is_object() {
                return;
            }
            doc.set(stored.clone());
            let ds = stored.get("docstatus").map(docstatus_rank).unwrap_or(0);
            docstatus.set(ds);
            baseline.set(fields.with_untracked(|f| payload_fingerprint(f)));
            // A lifecycle transition changes what is editable (a submitted
            // document locks its fields). The field views were built from the
            // previous model and are not rebuilt by replacing it, so re-read the
            // document once and let the load effect rebuild the form: the badge
            // used to flip to "Enviado" while the inputs stayed editable.
            if ds != 0 {
                reload.update(|n| *n += 1);
            }
        });

        // Set by `save()` when the write was a create, so the effect below knows
        // the `name` in `mutate_result` is a document worth navigating to. The
        // result is discarded otherwise: it is the only thing that carries the
        // name the engine assigned.
        let created = RwSignal::new(false);
        // Cloned before the effect takes it: calling a captured handle consumes
        // it, and both this effect and the duplicate action below need one.
        let navigate_after_create = navigate.clone();
        Effect::new(move |_| {
            let navigate = navigate_after_create.clone();
            let Some(stored) = saved_doc.get() else {
                return;
            };
            if !created.get() {
                return;
            }
            let Some(name) = stored.get("name").and_then(Value::as_str) else {
                return;
            };
            created.set(false);
            // The created document lives at a different route definition than
            // `/…/new`, so the router rebuilds the view, the load effect re-runs
            // with the real name, and `is_new` turns false. Without this the URL
            // stayed on `/…/new`, `is_new` stayed true and a second "Guardar"
            // created a second document.
            let url = format!(
                "/desk/{}/{}/{}",
                crate::desk::session::path_segment(&module.get()),
                crate::desk::session::path_segment(&doctype.get()),
                crate::desk::session::path_segment(&name)
            );
            navigate(&url, NavigateOptions::default());
        });

        // Save (create/update)
        let save_action = {
            let mutate = mutate.clone();
            Box::new(move || {
                saving.set(true);
                error.set(None);
                notice.set(None);
                if !validate_form(&fields.get()) {
                    let message = "Revisa los campos marcados".to_string();
                    set_form_error(error, message.clone());
                    saving.set(false);
                    // The inline banner says where, the toast says that the
                    // click did not save: a failed validation is a failed save,
                    // not a silent no-op.
                    last_write.set(Some(WriteOutcome::Failed(WriteVerb::Save, message)));
                    return;
                }
                let dt = doctype.get();
                let name = docname.get();
                let creating = is_new.get();
                // A Single is never "created": the save is an upsert through
                // `PUT /api/single/{doctype}` and the router stays put.
                let single = is_single.get();
                created.set(creating && !single);
                let payload =
                    strip_virtual_fields(build_form_payload(&fields.get()), meta.get().as_ref());
                // `saving` is deliberately left set here: the effect above clears
                // it when the request lands, which is the only moment the write
                // is actually over.
                let mut_type = if single {
                    MutationType::SaveSingle
                } else if creating {
                    MutationType::Create
                } else {
                    MutationType::Update
                };
                // Announced before `mutate` (which flips `mutating`
                // synchronously): the outcome effect needs both to agree.
                pending_write.set(Some(WriteVerb::Save));
                mutate(dt, name, Some(payload), mut_type);
            }) as Box<dyn Fn() + Send + Sync>
        };

        // Submit
        let submit_action = {
            let mutate = mutate.clone();
            Box::new(move || {
                pending_write.set(Some(WriteVerb::Submit));
                mutate(doctype.get(), docname.get(), None, MutationType::Submit);
            }) as Box<dyn Fn() + Send + Sync>
        };

        // Cancel
        let cancel_action = {
            let mutate = mutate.clone();
            Box::new(move || {
                pending_write.set(Some(WriteVerb::Cancel));
                mutate(doctype.get(), docname.get(), None, MutationType::Cancel);
            }) as Box<dyn Fn() + Send + Sync>
        };

        // Amend
        let amend_action = {
            let mutate = mutate.clone();
            Box::new(move || {
                pending_write.set(Some(WriteVerb::Amend));
                mutate(doctype.get(), docname.get(), None, MutationType::Amend);
            }) as Box<dyn Fn() + Send + Sync>
        };

        // Delete
        let delete_action = {
            let mutate = mutate.clone();
            Box::new(move || {
                pending_write.set(Some(WriteVerb::Delete));
                mutate(doctype.get(), docname.get(), None, MutationType::Delete);
            }) as Box<dyn Fn() + Send + Sync>
        };

        // Duplicate. A real copy this time: the engine assigns a new `name`, and
        // the router then follows it to the new document's own route. It does not
        // go through `use_mutation` because that hook derives the docname from
        // the route, and here the point is precisely to leave the current one.
        let duplicate_action = {
            let navigate = navigate.clone();
            Box::new(move || {
                // A form for a document that does not exist yet has nothing to
                // copy, and a document that is not a draft cannot become one by
                // being copied. Both gates are duplicated in the button; they are
                // repeated here because this is the one place that would corrupt
                // data if they were wrong.
                if is_new.get() || docstatus.get() != 0 || !allow_copy.get() {
                    return;
                }
                // The payload comes from the form, so copying before the load has
                // landed would create an empty document.
                if loading.get() {
                    return;
                }
                let dt = doctype.get();
                let module = module.get();
                let payload =
                    strip_virtual_fields(duplicate_payload(&fields.get()), meta.get().as_ref());
                saving.set(true);
                error.set(None);
                notice.set(None);
                // Cloned here, not inside the `async move`: moving the captured
                // handle into the future would make this closure `FnOnce`, and it
                // has to stay callable — the button can be pressed again.
                let navigate = navigate.clone();
                spawn_local(async move {
                    let store = match DocStore::runtime() {
                        Ok(s) => s,
                        Err(e) => {
                            error.set(Some(e.clone()));
                            saving.set(false);
                            last_write.set(Some(WriteOutcome::Failed(WriteVerb::Duplicate, e)));
                            return;
                        }
                    };
                    match store.create(&dt, payload).await {
                        Ok(stored) => {
                            saving.set(false);
                            match stored.get("name").and_then(Value::as_str) {
                                Some(name) => {
                                    last_write
                                        .set(Some(WriteOutcome::Succeeded(WriteVerb::Duplicate)));
                                    let url = format!(
                                        "/desk/{}/{}/{}",
                                        crate::desk::session::path_segment(&module),
                                        crate::desk::session::path_segment(&dt),
                                        crate::desk::session::path_segment(&name)
                                    );
                                    navigate(&url, NavigateOptions::default());
                                }
                                None => {
                                    let message = "La copia se creó pero la respuesta no traía \
                                                   nombre"
                                        .to_string();
                                    set_form_error(error, message.clone());
                                    last_write.set(Some(WriteOutcome::Failed(
                                        WriteVerb::Duplicate,
                                        message,
                                    )));
                                }
                            }
                        }
                        Err(e) => {
                            error.set(Some(e.clone()));
                            saving.set(false);
                            last_write.set(Some(WriteOutcome::Failed(WriteVerb::Duplicate, e)));
                        }
                    }
                });
            }) as Box<dyn Fn() + Send + Sync>
        };

        // `StoredValue` is non-reactive: it is written with `set_value`, not
        // with the reactive `set` an `RwSignal` uses (reactive_graph keeps the
        // two APIs apart on purpose).
        self.save_action.set_value(save_action);
        self.submit_action.set_value(submit_action);
        self.cancel_action.set_value(cancel_action);
        self.amend_action.set_value(amend_action);
        self.delete_action.set_value(delete_action);
        self.duplicate_action.set_value(duplicate_action);
    }
}
