//! Widget parameters as **form fields**: the properties panel renders each
//! `WidgetParam` through the same
//! [`FormField`](crate::views::services::FormField) model and the same
//! `crate::components::views::form::render_form_field` the document form
//! uses, instead of a private set of raw `<input>`/`<select>` controls.
//!
//! Why here and not in the panel: the translation `WidgetParam → FormField`
//! and back is policy, and a policy has exactly one home. Two rules are worth
//! reading twice:
//!
//! - **`value_type` is open, the editors are closed.** A widget may declare
//!   any string, so the mapping accepts (1) a canonical *fieldtype* the Desk
//!   can draw without a document context (`Link`, `Text`, `Select`…), then
//!   (2) the legacy kinds (`string`/`number`/`bool`/`doctype`/`json`) mapped
//!   to their fieldtype equivalent. Anything else degrades to `Data` — a
//!   control that always works, never one that needs `FormDocContext` (a
//!   `Button` or `Attachment Gallery` param would render a document-bound
//!   control in a dialog that has no document).
//! - **An untouched parameter is not materialized.** The runtime's
//!   `param_or` (widget_host) prefers a *stored* value over the
//!   definition's `default`, so writing the displayed value on "Aplicar"
//!   would freeze every default into the instance and break future default
//!   changes. [`sync_params`] therefore only writes what the user actually
//!   changed: an empty value removes the key, and an unchanged value with no
//!   stored key writes nothing.
//!
//! Split by concern: [`fieldtype`] owns the mapping and its allowlist,
//! [`panel`] builds the field list, [`sync`] writes it back.

mod fieldtype;
mod panel;
mod sync;

#[cfg(test)]
mod tests;

pub(super) use panel::param_fields;
pub(super) use sync::sync_params;
