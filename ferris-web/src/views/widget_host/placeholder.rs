//! Unresolved and misconfigured widget bodies: the placeholders that keep the
//! rest of the dashboard rendering.

use leptos::prelude::*;

use crate::ext::WidgetDef;

use super::params::HOST_OWNER;

/// An instance whose reference resolves to no definition *yet* (the
/// publishing app may register later — the server keeps the dashboard for
/// the same reason): a dashed placeholder that says so, with no mount anchor.
/// The dashed frame marks a placeholder, not a widget card; the host header
/// still carries the title.
pub(super) fn unresolved_widget(reference: &str) -> AnyView {
    view! {
        <div class="rounded-md border border-dashed p-3 text-xs text-muted-foreground">
            "Widget pendiente: " <code>{reference.to_string()}</code>
        </div>
    }
    .into_any()
}

/// A resolved widget whose `Widget` row has no code and no native renderer:
/// the seed never migrated a module for it. Naming the reference is more
/// useful than mounting nothing, and it keeps the rest of the dashboard alive.
pub(super) fn missing_code_widget(reference: &str) -> AnyView {
    view! {
        <div class="rounded-md border border-dashed p-3 text-xs text-muted-foreground">
            "Widget sin código: " <code>{reference.to_string()}</code>
        </div>
    }
    .into_any()
}

/// A host widget whose required param (like `doctype`) was never set: name
/// the missing one instead of asking the DocStore for a doctype-less list
/// that can only fail.
pub(super) fn missing_param(def: &WidgetDef, missing: &str) -> AnyView {
    view! {
        <div class="rounded-md border border-dashed p-3 text-xs text-muted-foreground">
            "Falta el parámetro "
            <code>{missing.to_string()}</code>
            " de "
            <code>{format!("{HOST_OWNER}.{}", def.name)}</code>
        </div>
    }
    .into_any()
}

/// A `host.*` name with no native renderer (the catalog gained one this
/// Desk predates): the placeholder says so instead of mounting nothing.
pub(super) fn unknown_host_widget(def: &WidgetDef) -> AnyView {
    view! {
        <div class="rounded-md border border-dashed p-3 text-xs text-muted-foreground">
            "Renderer nativo desconocido para "
            <code>{format!("{HOST_OWNER}.{}", def.name)}</code>
        </div>
    }
    .into_any()
}
