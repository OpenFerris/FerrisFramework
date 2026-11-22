//! The form's contribution to the Desk's shared top bar.
//!
//! The Desk has exactly one top bar — the registry-based one the list view
//! publishes into (`ferris_web_ui::components::desk::top_bar`). The form used to
//! draw a second bar of its own inside the page, which duplicated the
//! breadcrumbs and put an action row in the content; this module feeds the same
//! registry instead, so the breadcrumbs, the status indicator and the
//! mode-dependent actions live in one place and every route looks alike.
//!
//! The arrangement follows Frappe's form page head:
//!
//! * the **primary action** reflects the document state — Enviar → Guardar →
//!   Actualizar → Cancelar → Amendar, in Frappe's order of precedence;
//! * **prev/next** walk the list, and are only offered for a stored, non-Single
//!   document;
//! * a single **"⋯" menu** holds the secondary actions, grouped with dividers.
//!
//! A new document offers nothing but the primary action: Frappe hides both its
//! menu and its navigation for a not-yet-saved document, and so does this bar.
//! The entries the backend cannot back yet (email, remind, documentation) are
//! rendered **disabled and labelled**, never as a click that silently does
//! nothing (AGENTS.md §4: no silent success).
//!
//! # Callback lifetime — the rule this module exists to respect
//!
//! A `Callback` is a `StoredValue`, so it dies with the reactive owner that
//! created it — and *every* owner run disposes what its previous run owned:
//! both `Effect::new` (each pass runs under `Owner::with_cleanup`) and a
//! derived signal re-dispose their previous generation. A callback created
//! inside the action effect was therefore already disposed by the next state
//! change, while the top bar still drew the previous generation; clicking it
//! panicked the whole wasm module ("reactive value … already been disposed" —
//! the incident behind [`form_top_bar::FormActions`]).
//!
//! The rule: the action effect builds only **specs** (labels, icons, which
//! button is showing) over callbacks created **once in the component body**,
//! whose owner is the same one whose cleanup empties the registry — so the
//! slot holds live callbacks by construction. The top bar itself additionally
//! invokes every action with `try_run`, so even a handler that outlives its
//! view degrades to a no-op instead of a panic.
//!
//! # The pieces
//!
//! * [`actions`] — the specs for the current state (primary action, navigation,
//!   the "⋯" menu).
//! * [`menu`] — the DocType-declared actions the menu adds to the built-ins.
//! * [`export_dialog`] — "Exportar fixtures", the only write the layout editor
//!   performs on a plugin world.
//! * [`leave_dialog`] — the confirmation before leaving the layout session.
//! * [`leave_guard`] — the link/unload half of that confirmation (browser only).
//! * [`toast`] — the transient report every action emits (the shell owns the
//!   toast host; this is only the handle).
//! * [`form_top_bar`] — the component and the once-per-mount callbacks.
//! * [`shortcuts`] — the `Ctrl+Z` / `Ctrl+Shift+Z` listener for layout edits.

mod actions;
mod dialogs;
mod export_dialog;
mod form_top_bar;
mod leave_dialog;
mod leave_guard;
mod menu;
mod rename_dialog;
mod shortcuts;
mod toast;

#[cfg(test)]
mod menu_tests;

pub use form_top_bar::FormTopBar;
pub use menu::{FormMenuAction, FormMenuKind, menu_actions};
