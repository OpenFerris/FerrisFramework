//! The document identity and feedback channel the action-bearing fieldtypes
//! need.
//!
//! A `Button` field calls `/api/method/<options>` on the document being
//! edited, and an `Attachment Gallery` lists that document's files: neither
//! can work from a `FormField` alone. The form view provides this context once
//! (see `ferris-web/src/views/form_view.rs`), the widgets read it with
//! `use_context`, and no other part of the form has to know they exist.

use leptos::prelude::*;
use serde_json::Value;

/// Identity, raw document and feedback for fieldtypes that act on the whole
/// document (`Button`, `Attachment Gallery`).
///
/// Every field is `Copy` (Leptos 0.8 signals are), so the context itself is
/// cheap to clone per widget.
#[derive(Clone, Copy)]
pub struct FormDocContext {
    pub doctype: Signal<String>,
    pub docname: Signal<String>,
    pub is_new: Memo<bool>,
    /// The raw document as the API returned it (`Value::Null` on a `new`
    /// form): what a `Button` field hands to its action.
    pub doc: ReadSignal<Value>,
    /// The banner the form view already renders; widgets report their outcome
    /// here instead of inventing their own toast.
    pub notice: RwSignal<Option<String>>,
    /// Bumped to make the form reload — a `Button` action that changes the
    /// document server-side refreshes the view through this.
    pub reload: RwSignal<u64>,
}
