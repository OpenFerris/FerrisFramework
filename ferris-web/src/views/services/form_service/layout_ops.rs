//! Undo/redo for *layout* edits: the client-owned half of the customization
//! editor.
//!
//! A layout edit is a `Custom Field` or `Property Setter` write; the server
//! stores only the result, never the intent, so there is nothing to diff
//! against later. That is why undo cannot be reconstructed from the source of
//! truth: the stack below is the only record of "what *this* client changed",
//! and it exists exactly as long as this form does. Reopening the form starts
//! empty, which is the honest behaviour — a stack rebuilt from the server would
//! happily "undo" a change somebody else made.
//!
//! Each [`LayoutOp`] therefore stores the **inverse**, not the change: enough
//! data to revert it (`undo_write`) and enough to re-apply it (`redo_write`).
//! Both directions are pure functions, tested in `layout_ops_tests`, so the
//! async half only has to pick a `DocStore` call and report its result through
//! the same [`WriteOutcome`] channel the lifecycle actions use.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use crate::api::types::{CustomFieldSpec, PropertySetterSpec};

use super::layout_history::{self, LayoutHistory};
use super::{FormService, WriteOutcome, WriteVerb};

/// One layout edit this client performed, invertible.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutOp {
    /// A custom field the client created. Undoing deletes it; redoing re-creates
    /// it from the same spec, which is why the whole spec is kept instead of
    /// just the fieldname.
    FieldCreated { spec: CustomFieldSpec },
    /// A property setter the client wrote. `previous` is what it replaced, and
    /// `None` means there was no setter for that `(fieldname, property)` pair —
    /// undoing is then a delete, not a restore to an empty string.
    PropertySet {
        fieldname: Option<String>,
        property: String,
        previous: Option<String>,
        value: String,
    },
}

/// The single `DocStore` write that reverts (or re-applies) a [`LayoutOp`].
///
/// A named value instead of an `async` closure so the mapping stays a pure
/// function: it is the part worth testing, and the part a new kind of edit has
/// to extend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpWrite {
    PutField(CustomFieldSpec),
    DeleteField {
        fieldname: String,
    },
    PutProperty(PropertySetterSpec),
    DeleteProperty {
        fieldname: Option<String>,
        property: String,
    },
}

impl LayoutOp {
    /// The write that takes this edit back.
    #[must_use]
    pub fn undo_write(&self) -> OpWrite {
        match self {
            LayoutOp::FieldCreated { spec } => OpWrite::DeleteField {
                fieldname: spec.fieldname.clone(),
            },
            LayoutOp::PropertySet {
                fieldname,
                property,
                previous,
                ..
            } => match previous {
                Some(value) => OpWrite::PutProperty(PropertySetterSpec {
                    fieldname: fieldname.clone(),
                    property: property.clone(),
                    value: value.clone(),
                }),
                None => OpWrite::DeleteProperty {
                    fieldname: fieldname.clone(),
                    property: property.clone(),
                },
            },
        }
    }

    /// The write that performs the edit again, after an undo.
    #[must_use]
    pub fn redo_write(&self) -> OpWrite {
        match self {
            LayoutOp::FieldCreated { spec } => OpWrite::PutField(spec.clone()),
            LayoutOp::PropertySet {
                fieldname,
                property,
                value,
                ..
            } => OpWrite::PutProperty(PropertySetterSpec {
                fieldname: fieldname.clone(),
                property: property.clone(),
                value: value.clone(),
            }),
        }
    }
}

impl FormService {
    /// Record an edit this client just performed, so it can be undone.
    ///
    /// Also clears the redo branch: after a new edit, "redo" would re-apply a
    /// change that is no longer the next step, which is the usual editor
    /// contract and the only one that cannot produce an inconsistent layout.
    ///
    /// Record **after** the write succeeded. An edit the server rejected never
    /// happened, and putting it on the stack would make undo delete something
    /// that was not created.
    pub fn record_layout_op(&self, op: LayoutOp) {
        self.undo_stack.update(|stack| stack.push(op));
        self.redo_stack.update(Vec::clear);
        self.persist_history();
    }

    /// Mirror the live stacks into the session store, so the next mount of the
    /// **editor** for this `DocType` still offers undo.
    fn persist_history(&self) {
        let doctype = self.doctype.get_untracked();
        if doctype.is_empty() {
            return;
        }
        layout_history::store(
            &doctype,
            LayoutHistory {
                undo: self.undo_stack.get_untracked(),
                redo: self.redo_stack.get_untracked(),
            },
        );
    }

    /// End the layout session: forget the recorded edits and the store entry.
    ///
    /// Called when the editor is left (and by the leave confirmation before it
    /// navigates). The layout stays applied on the server — these stacks only
    /// ever held *this client's* undo history.
    pub fn discard_layout_history(&self) {
        let doctype = self.doctype.get_untracked();
        if !doctype.is_empty() {
            layout_history::clear(&doctype);
        }
        self.undo_stack.set(Vec::new());
        self.redo_stack.set(Vec::new());
    }

    /// Whether the editor holds layout edits this session could undo.
    #[must_use]
    pub fn has_layout_history(&self) -> bool {
        !self.undo_stack.with_untracked(Vec::is_empty)
            || !self.redo_stack.with_untracked(Vec::is_empty)
    }

    /// Whether there is an edit to take back.
    ///
    /// False outside the layout session: the stacks are only that session's, so
    /// no document view can offer (or run) an undo it has no surface for.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.layout_mode.get_untracked() && !self.undo_stack.with_untracked(Vec::is_empty)
    }

    /// Whether there is an undone edit to re-apply. Layout session only, like
    /// [`FormService::can_undo`].
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.layout_mode.get_untracked() && !self.redo_stack.with_untracked(Vec::is_empty)
    }

    /// Revert the newest recorded edit.
    pub fn undo_layout(&self) {
        if self.layout_mode.get_untracked() {
            self.step_layout(false);
        }
    }

    /// Re-apply the newest undone edit.
    pub fn redo_layout(&self) {
        if self.layout_mode.get_untracked() {
            self.step_layout(true);
        }
    }

    /// Move `fieldname` immediately before `before` (drag and drop on the form).
    ///
    /// The move is an `insert_after` property setter anchored at the field that
    /// currently precedes the target (`__first__` when the target is first), so
    /// the engine applies it to the effective metadata and a compiled field
    /// moves just like a custom one. The setter that was in force is read before
    /// the write, which is what makes the move undoable to the exact previous
    /// position.
    pub fn move_field_before(&self, fieldname: String, before: String) {
        let Some(meta) = self.meta.get_untracked() else {
            return;
        };
        let names: Vec<String> = meta.fields.iter().map(|f| f.fieldname.clone()).collect();
        let Some(anchor) = drop_anchor(&names, &fieldname, &before) else {
            // Nothing to do: the field already sits there, or one of the two is
            // gone from the metadata.
            return;
        };
        let doctype = self.doctype.get_untracked();
        let service = self.clone();
        spawn_local(async move {
            let store = match DocStore::runtime() {
                Ok(store) => store,
                Err(message) => {
                    service
                        .last_write
                        .set(Some(WriteOutcome::Failed(WriteVerb::Move, message)));
                    return;
                }
            };
            let previous = match store.get_customization(&doctype).await {
                Ok(custom) => custom
                    .properties
                    .iter()
                    .find(|setter| {
                        setter.fieldname.as_deref() == Some(fieldname.as_str())
                            && setter.property == "insert_after"
                    })
                    .map(|setter| setter.value.clone()),
                Err(message) => {
                    service
                        .last_write
                        .set(Some(WriteOutcome::Failed(WriteVerb::Move, message)));
                    return;
                }
            };
            let spec = PropertySetterSpec {
                fieldname: Some(fieldname.clone()),
                property: "insert_after".to_string(),
                value: anchor.clone(),
            };
            match store.put_property_setter(&doctype, &spec).await {
                Ok(_) => {
                    service.record_layout_op(LayoutOp::PropertySet {
                        fieldname: Some(fieldname),
                        property: "insert_after".to_string(),
                        previous,
                        value: anchor,
                    });
                    // The effective order is rebuilt from the setters, so the
                    // form has to re-read to paint the new position.
                    service.reload();
                    service
                        .last_write
                        .set(Some(WriteOutcome::Succeeded(WriteVerb::Move)));
                }
                Err(message) => service
                    .last_write
                    .set(Some(WriteOutcome::Failed(WriteVerb::Move, message))),
            }
        });
    }

    /// Move one operation between the stacks by writing its inverse.
    ///
    /// The stacks are only mutated on success. A failed write leaves the
    /// operation where it was, so the user can retry it instead of losing the
    /// only record of a change that is still applied on the server.
    fn step_layout(&self, redo: bool) {
        let (from, to) = if redo {
            (self.redo_stack, self.undo_stack)
        } else {
            (self.undo_stack, self.redo_stack)
        };
        let Some(op) = from.with_untracked(|stack| stack.last().cloned()) else {
            return;
        };
        let verb = if redo {
            WriteVerb::Redo
        } else {
            WriteVerb::Undo
        };
        let write = if redo {
            op.redo_write()
        } else {
            op.undo_write()
        };
        let doctype = self.doctype.get_untracked();
        let service = self.clone();
        spawn_local(async move {
            let store = match DocStore::runtime() {
                Ok(store) => store,
                Err(message) => {
                    service
                        .last_write
                        .set(Some(WriteOutcome::Failed(verb, message)));
                    return;
                }
            };
            let result = match &write {
                OpWrite::PutField(spec) => store.put_custom_field(&doctype, spec).await.map(|_| ()),
                OpWrite::DeleteField { fieldname } => {
                    store.delete_custom_field(&doctype, fieldname).await
                }
                OpWrite::PutProperty(spec) => {
                    store.put_property_setter(&doctype, spec).await.map(|_| ())
                }
                OpWrite::DeleteProperty {
                    fieldname,
                    property,
                } => {
                    store
                        .delete_property_setter(&doctype, fieldname.as_deref(), property)
                        .await
                }
            };
            match result {
                Ok(()) => {
                    from.update(|stack| {
                        stack.pop();
                    });
                    to.update(|stack| stack.push(op));
                    service.persist_history();
                    // The fields were built from the previous metadata; the
                    // engine applies customizations on the next read, so the
                    // form has to re-read to show the reverted shape.
                    service.reload();
                    service.last_write.set(Some(WriteOutcome::Succeeded(verb)));
                }
                Err(message) => service
                    .last_write
                    .set(Some(WriteOutcome::Failed(verb, message))),
            }
        });
    }
}

/// The `insert_after` anchor that puts `name` immediately before `before`.
///
/// `None` means "no write": an unknown field, the same field twice, or a field
/// that already sits exactly there (anchored on its own predecessor). Pure so
/// the drop arithmetic is testable without a server.
#[must_use]
pub(super) fn drop_anchor(names: &[String], name: &str, before: &str) -> Option<String> {
    if name == before || !names.iter().any(|candidate| candidate == name) {
        return None;
    }
    let target = names.iter().position(|candidate| candidate == before)?;
    let anchor = if target == 0 {
        "__first__".to_string()
    } else {
        names[target - 1].clone()
    };
    // The dragged field already is the target's predecessor: the write would
    // change nothing and would still spend an undo slot.
    (anchor != name).then_some(anchor)
}
