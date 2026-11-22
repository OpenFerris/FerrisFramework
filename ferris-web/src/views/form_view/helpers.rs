//! Decisions that select what a document route paints before (and instead of)
//! the metadata form.
//!
//! Split from `mod.rs` because the form body is at the repository's file-size
//! budget, and because these three items are one concern: the *outer* phase
//! (loading / failed / ready) and the two declared-view branches (dashboard and
//! a custom view nobody answered). Keeping them together means a change to the
//! dispatch rule is one reviewable file, not a hunt through the layout.

use leptos::prelude::*;

use ferris_schema::contract::DocView;

use crate::views::services::FormService;

/// The outer state of the form: what the page has to paint before the editor
/// exists. A `Memo` over this value is what keeps the `Ready` subtree mounted
/// across metadata reloads (see [`super::MetadataFormView`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FormPhase {
    Loading,
    Failed,
    Ready,
}

/// Whether this load opens as the dashboard builder instead of the metadata
/// form. Single source of truth for the branch in [`super::MetadataFormView`]
/// *and* for its wrapper's width: a dashboard owns the full viewport width, the
/// form is a centred `max-w-7xl` column, and two copies of this condition are
/// how the shell ends up capping the dashboard.
///
/// Mirrors the form branch's gates exactly: during a load, after a failed
/// read, or before the record exists, what the user sees is the skeleton, the
/// error banner or a blank form — all of which stay inside the narrow column.
pub(super) fn opens_as_dashboard(svc: &FormService) -> bool {
    if svc.loading.get() {
        return false;
    }
    // A load that failed is not a dashboard: `fields` stays empty when the
    // metadata or the document could not be read.
    if svc.error.get().is_some() && svc.fields.get().is_empty() {
        return false;
    }
    // `new` is never a declared view: creating a document is the form's job
    // even for a dashboard doctype (the record has to exist before it can
    // open as one).
    if svc.is_new.get() {
        return false;
    }
    svc.meta
        .get()
        .is_some_and(|meta| meta.view == DocView::Dashboard)
}

/// [`DocView::Custom`] with nothing to mount: the doctype refused the
/// built-in form, so drawing it anyway would quietly contradict the
/// declaration — a doctype that says "custom view" and gets the metadata form
/// is the same defect as a doctype that says "dashboard" and gets it.
///
/// Reaching this is only possible *after* [`super::FormView`] has looked for
/// the feed's `form.view` element and found none: when an app declares one,
/// that element owns the route and this view never runs.
pub(super) fn custom_view_missing() -> impl IntoView {
    view! {
        <div class="rounded-xl border p-6 space-y-2" data-name="DoctypeCustomViewMissing">
            <div class="text-sm font-medium">"Esta doctype declara una vista personalizada"</div>
            <p class="text-sm text-muted-foreground">
                "Ninguna app publica su elemento, así que no hay nada que abrir. "
                "Instala la app que la define o cambia la declaración de la vista de la doctype."
            </p>
        </div>
    }
}
