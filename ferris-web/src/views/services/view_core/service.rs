//! The behaviour every view gets from its `ViewCore`.
//!
//! A trait instead of `Deref` on purpose: a concrete view composes a core and
//! opts into these operations explicitly, so a method cannot silently resolve
//! against the wrong layer.

use std::collections::HashSet;

use leptos::prelude::*;
use serde_json::Value;

use super::ViewCore;
use crate::api::Filter;
use crate::api::meta::Column;

// ────────────────────────────────────────────────────────────────────────────
// ViewService — contrato común, sin Deref.
// ────────────────────────────────────────────────────────────────────────────

pub trait ViewService: Clone + 'static {
    /// Acceso al núcleo. Único método obligatorio.
    fn core(&self) -> &ViewCore;

    // ── Identidad ───────────────────────────────────────────────────────
    // Signals like the rest of this trait: the view outlives a single
    // doctype, so a `&str` borrowed out of the service
    // would only ever describe the doctype it was built with.
    fn doctype(&self) -> Signal<String> {
        self.core().doctype
    }
    fn module(&self) -> Signal<String> {
        self.core().module
    }
    fn view_id(&self) -> &'static str {
        self.core().view_id
    }

    // ── Datos ───────────────────────────────────────────────────────────
    fn rows(&self) -> ReadSignal<Vec<Value>> {
        self.core().rows
    }
    fn total(&self) -> ReadSignal<u64> {
        self.core().total
    }
    fn loading(&self) -> ReadSignal<bool> {
        self.core().loading
    }
    fn error(&self) -> ReadSignal<Option<String>> {
        self.core().error
    }
    /// Whether another page is being appended (infinite list).
    fn loading_more(&self) -> Signal<bool> {
        self.core().loading_more
    }
    /// Whether the loaded rows are still a prefix of the server's result set.
    fn has_more(&self) -> Signal<bool> {
        self.core().has_more
    }
    /// Ask the core for the next page. A no-op for non-infinite views.
    fn load_more(&self) {
        self.core().load_more.run(());
    }

    // ── Metadata ────────────────────────────────────────────────────────
    fn meta(&self) -> RwSignal<Option<crate::api::DynMeta>> {
        self.core().meta
    }

    // ── Acciones ────────────────────────────────────────────────────────
    fn reload(&self) {
        self.core().reload.run(());
    }
    fn set_filters(&self, f: Vec<Filter>) {
        self.core().set_filters(f);
    }
    fn on_search(&self, term: String) {
        self.core().set_search(term);
    }

    // ── Selección ───────────────────────────────────────────────────────
    fn selected(&self) -> RwSignal<HashSet<String>> {
        self.core().selected
    }
    fn selected_count(&self) -> Memo<usize> {
        self.core().selected_count
    }
    fn page_names(&self) -> Memo<Vec<String>> {
        self.core().page_names
    }
    fn all_page_checked(&self) -> Memo<bool> {
        self.core().all_page_checked
    }
    fn selected_names(&self) -> Vec<String> {
        self.core().selected_names()
    }
    fn clear_selection(&self) {
        self.core().clear_selection();
    }
    fn toggle_row(&self, name: String, on: bool) {
        self.core().toggle_row(name, on);
    }
    fn toggle_all_on_page(&self, on: bool) {
        self.core().toggle_all_on_page(on);
    }

    // ── Bulk ────────────────────────────────────────────────────────────
    fn bulk_delete(&self) {
        self.core().bulk_delete.run(());
    }
    fn bulk_export(&self, cols: Vec<Column>) {
        self.core().bulk_export.run(cols);
    }
    fn export_page(&self, cols: Vec<Column>) {
        self.core().export_page.run(cols);
    }

    // ── Mutaciones (feedback al UI) ─────────────────────────────────────
}
