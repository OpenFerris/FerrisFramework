//! The catalog's card pieces: the theme-aware preview and the widget tile.
//! Split from [`mod`](super::drawer) (the two-level shell) so each file stays
//! well under the 600-line rule.
//!
//! Every card shows a real image, never a bare icon: [`preview::preview_src`]
//! resolves `WidgetDef::preview` (WIT 9.0.0) → the shipped fallback for the
//! widget's `kind` → `generic.svg`, and [`preview::PreviewImg`] renders it —
//! as an `<img>` first, then inlined in a shadow root so it follows the Desk's
//! runtime theme (see [`preview`] for why an `<img>` alone cannot).

use leptos::prelude::*;

use ferris_schema::contract::WidgetDef;

use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};

use super::super::preview::{PreviewImg, preview_src};

/// How a card titles a definition: its label, or `owner.name` when the app
/// gave none (the shape the palette has always used).
pub(super) fn card_title(owner: &str, def: &WidgetDef) -> String {
    if def.label.is_empty() {
        format!("{owner}.{}", def.name)
    } else {
        def.label.clone()
    }
}

/// The preview image of a definition. The resolved source is reactive, so a
/// feed refresh that changes `preview` re-runs the inline mount instead of
/// keeping the stale SVG.
pub(super) fn preview_any(title: &str, def: &WidgetDef) -> AnyView {
    let alt = format!("Vista previa de {title}");
    let src = preview_src(def);
    let source = Signal::derive(move || src.clone());
    view! { <PreviewImg alt=alt source=source /> }.into_any()
}

/// One widget card: header (title + publisher badge) over content (the
/// preview image). The whole card is the add button — clicking it places an
/// instance and the shelf closes the panel through `on_add`'s wrapper.
pub(super) fn widget_card(
    owner: String,
    def: WidgetDef,
    on_add: Callback<(String, WidgetDef)>,
) -> AnyView {
    let title = card_title(&owner, &def);
    let preview = preview_any(&title, &def);
    // The click closure moves `owner`; the badge needs its own copy.
    let badge_owner = owner.clone();
    view! {
        <li>
            <button
                type="button"
                data-name="__WidgetCard"
                class="flex w-full flex-col overflow-hidden rounded-lg border border-border bg-card text-left transition-colors hover:bg-accent/50 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                on:click=move |_| on_add.run((owner.clone(), def.clone()))
            >
                <span class="flex items-center justify-between gap-2 border-b border-border px-3 py-2">
                    <span class="min-w-0 truncate text-sm font-medium">{title}</span>
                    <Badge variant=BadgeVariant::Secondary size=BadgeSize::Sm>{badge_owner}</Badge>
                </span>
                <span class="flex justify-center bg-background p-2">{preview}</span>
            </button>
        </li>
    }
    .into_any()
}
