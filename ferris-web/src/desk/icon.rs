//! The icon a Desk tile draws: either a named icon or an app-declared SVG.
//!
//! A workspace/folder `icon` is a plain string on the wire. Historically it was
//! always an icon **name** (`"Boxes"`, `"book-open"`). It may now also be an SVG
//! **document** (`"<svg …>…</svg>"`), so this component routes the two shapes:
//! names go through the shared [`Icon`](crate::components::ui::icon::Icon)
//! resolution, SVG markup is sanitized and inlined by [`crate::svg`].
//!
//! The fallback matters for SSR and hydration parity: the server cannot run the
//! DOM sanitizer, so it renders an isolated `<img>` with the SVG as a data URI
//! (script-free, no external fetches). The post-hydration effect swaps in the
//! sanitized inline version, which is the one that inherits `currentColor` and
//! the active theme's tokens.

use leptos::prelude::*;

use crate::components::ui::icon::Icon;

/// One Desk tile glyph: an icon name or inline SVG markup, with a shared class
/// that sets its size.
#[component]
pub fn DeskGlyph(#[prop(into)] icon: Option<String>, #[prop(into)] class: String) -> impl IntoView {
    let icon = icon
        .as_deref()
        .map(str::trim)
        .filter(|icon| !icon.is_empty());
    match icon {
        Some(markup) if crate::svg::looks_like_svg(markup) => {
            view! { <InlineSvg markup=markup.to_string() class=class /> }.into_any()
        }
        Some(name) => view! { <Icon name=name.to_string() class=class /> }.into_any(),
        None => ().into_any(),
    }
}

/// The SVG half of [`DeskGlyph`]: a box the caller sizes, an `<img>` fallback
/// for SSR/first paint, and the sanitized inline SVG once hydrated.
#[component]
fn InlineSvg(markup: String, class: String) -> impl IntoView {
    let host = NodeRef::<leptos::html::Span>::new();
    let source = StoredValue::new(markup);
    let fallback = crate::svg::data_uri(&source.get_value());

    Effect::new(move |_| {
        let Some(node) = host.get() else {
            return;
        };
        crate::svg::mount_inline(&node, &source.get_value());
    });

    view! {
        // `inline-flex` matters: `size-*` on an inline `<span>` sets no box, and
        // the SSR `<img>` fill would collapse to the SVG's intrinsic size.
        <span node_ref=host class=format!("inline-flex shrink-0 {class}") aria-hidden="true">
            <img src=fallback alt="" class="size-full" />
        </span>
    }
}
