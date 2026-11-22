//! Theme-aware widget previews (D3.3 of the dashboard widget).
//!
//! A catalog card shows its widget's preview image. An `<img src="…svg">` is an
//! **isolated document**: it cannot read the Desk's `var(--token)`, so a preview
//! loaded that way can only ever follow `prefers-color-scheme`, never the user's
//! runtime theme. The fix is to fetch the SVG and place it **inline inside a
//! shadow root**: CSS custom properties inherit through the shadow boundary, so
//! the SVG's own `var(--card)` / `var(--foreground)` resolve against whatever
//! theme the Desk has painted on `:root`/`.dark` right now — light, dark, or a
//! plugin theme. That is why the shadow root is not optional here: it is both
//! the isolation and the inheritance channel.
//!
//! Inlining third-party markup is the risk this module exists to bound: the
//! preview is a `/plugins/{owner}/asset/…` URL, i.e. an app-authored file. It is
//! sanitized with the platform DOM **before** it reaches the shadow root —
//! [`crate::svg::sanitize_svg`] rejects a non-`<svg>` root and strips scripting
//! and external references, so an app cannot turn a catalog tile into script
//! execution. The shadow root then keeps the SVG's own `<style>` from leaking
//! onto the Desk.
//!
//! SSR and hydration are untouched: the server (and the first hydrate pass)
//! render the plain `<img>`; the inline swap happens in a post-hydration effect.
//! If the fetch or the sanitizer fails, the `<img>` stays and keeps its
//! `on:error` fallback — a card is never a silent hole.

use leptos::prelude::*;

use ferris_schema::contract::WidgetDef;
#[cfg(target_arch = "wasm32")]
use ferris_web_shared::html::escape_html;

/// Where the generic fallback lives. Also the `<img>` `on:error` target: a
/// preview that 404s (or a stale URL from an app) ends on this image, which the
/// server always has.
pub(super) const GENERIC_PREVIEW: &str = "/static/previews/generic.svg";

/// The image for `def`: its declared `preview` when it has one, else the
/// shipped fallback for its `kind`, else [`GENERIC_PREVIEW`]. It is the
/// `<img>`'s `src`, the URL the inline path fetches, and the fallback chain in
/// one place, so the three can never disagree.
pub(super) fn preview_src(def: &WidgetDef) -> String {
    if let Some(url) = def.preview.as_deref().filter(|url| !url.is_empty()) {
        return url.to_string();
    }
    match def.kind.as_str() {
        "stat" => "/static/previews/stat.svg",
        "chart" => "/static/previews/chart.svg",
        "table" => "/static/previews/table.svg",
        _ => GENERIC_PREVIEW,
    }
    .to_string()
}

/// Inline, theme-aware preview of a catalog card.
///
/// Server and first-hydrate markup is the plain `<img>` (fast, no flash of
/// empty box, works without JS). A post-hydration effect then fetches the SVG,
/// sanitizes it and swaps it in inside its own shadow root, where the Desk's
/// custom properties reach it. Any failure — fetch, sanitizer, a root that is
/// not an `<svg>` — leaves the `<img>` exactly as it was, still guarded by
/// `on:error`.
///
/// `source` is a signal so a feed refresh that changes the URL re-runs the
/// effect; the effect clears the previous inline tree before mounting the new
/// one, so a card never shows two previews at once.
#[component]
pub(super) fn PreviewImg(alt: String, source: Signal<String>) -> impl IntoView {
    let failed: RwSignal<bool> = RwSignal::new(false);
    let host: NodeRef<leptos::html::Span> = NodeRef::new();
    // The URL the `<img>` resolves to, generic after a load error.
    let img_src = move || {
        if failed.get() {
            GENERIC_PREVIEW.to_string()
        } else {
            source.get()
        }
    };
    let inline_alt = alt.clone();
    Effect::new(move |_| {
        // Subscribe to the URL: a new source clears the old inline tree and
        // starts the fetch again.
        let target = source.get();
        let Some(node) = host.get() else {
            return; // SSR / pre-hydration: the `<img>` is the answer.
        };
        reset(&node);
        let alt = inline_alt.clone();
        crate::api::drive(async move {
            let Ok(html) = crate::api::client::api_get_raw(&target).await else {
                return; // `<img>` keeps its own (possibly fallback) source.
            };
            inline_html(&node, &html, &alt);
        });
    });
    view! {
        <span
            node_ref=host
            class="flex aspect-[4/3] w-full items-center justify-center overflow-hidden rounded border border-border bg-card"
        >
            <img
                src=img_src
                alt=alt
                loading="lazy"
                class="h-full w-full object-contain"
                on:error=move |_| failed.set(true)
            />
        </span>
    }
}

/// Client-only entry point for the inline swap. On `wasm32` it sanitizes and
/// mounts; on any other target the effect is unreachable in practice (SSR never
/// runs it against a live DOM), so it is a no-op and keeps the call site free of
/// `cfg` noise.
#[cfg(target_arch = "wasm32")]
fn inline_html(host: &web_sys::Element, html: &str, alt: &str) {
    mount_svg(host, html, alt);
}

/// Non-wasm stub: no DOM exists to mount into.
#[cfg(not(target_arch = "wasm32"))]
fn inline_html<H>(_host: &H, _html: &str, _alt: &str) {}

/// Drop whatever the previous inline mount left in `host`'s shadow root and
/// re-expose the `<img>` (a new source is about to be fetched).
#[cfg(target_arch = "wasm32")]
fn reset(host: &web_sys::Element) {
    if let Some(root) = host.shadow_root() {
        root.set_inner_html("");
    }
    if let Ok(Some(img)) = host.query_selector("img") {
        let _ = img.remove_attribute("hidden");
        let _ = img.remove_attribute("aria-hidden");
    }
}

/// Non-wasm stub: nothing was mounted.
#[cfg(not(target_arch = "wasm32"))]
fn reset<H>(_host: &H) {}

/// Attach (or reuse) a preview's shadow root on `host` and place the sanitized
/// SVG inside it. Does nothing when `html` is not an inlinable SVG, so the
/// `<img>` the caller already rendered stays.
#[cfg(target_arch = "wasm32")]
fn mount_svg(host: &web_sys::Element, html: &str, alt: &str) {
    let Some(clean) = crate::svg::sanitize_svg(html) else {
        return;
    };
    let Some(root) = crate::ext::loader::shadow_root(host) else {
        return;
    };
    // The SVG carries no `width`/`height`; the host box (set by `PreviewImg`)
    // sizes it, so the markup is wrapped once and styled to fill the box.
    let frame = format!(
        "<style>\
           :host {{ display:block; width:100%; height:100%; }}\
           svg, .ferris-widget-preview {{ display:block; width:100%; height:100%; }}\
         </style>\
         <span role=\"img\" aria-label=\"{}\" class=\"ferris-preview-host\" \
           style=\"display:block;width:100%;height:100%\">{clean}</span>",
        escape_html(alt)
    );
    root.set_inner_html(&frame);
    // The `<img>` sibling is now redundant: hide it so the two never show at
    // once, and drop it from the a11y tree to avoid a duplicate image.
    if let Some(img) = host.query_selector("img").ok().flatten() {
        let _ = img.set_attribute("hidden", "");
        let _ = img.set_attribute("aria-hidden", "true");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::preview_src;
    use ferris_schema::contract::WidgetDef;

    fn def(preview: Option<&str>, kind: &str) -> WidgetDef {
        WidgetDef {
            name: "w".into(),
            label: "W".into(),
            component: "dashboard/w".into(),
            kind: kind.into(),
            params: vec![],
            description: None,
            requires: None,
            events: vec![],
            icon: None,
            category: None,
            preview: preview.map(str::to_string),
            code_url: None,
            style_url: None,
            size: None,
            min: None,
            style: None,
        }
    }

    /// A declared preview wins; otherwise the `kind` fallback; otherwise the
    /// generic image. The three can never disagree because this is the only
    /// place that decides.
    #[test]
    fn preview_src_falls_back_by_kind() {
        assert_eq!(
            preview_src(&def(Some("/plugins/a/asset/p.svg"), "chart")),
            "/plugins/a/asset/p.svg"
        );
        assert_eq!(
            preview_src(&def(Some(""), "chart")),
            "/static/previews/chart.svg"
        );
        assert_eq!(preview_src(&def(None, "stat")), "/static/previews/stat.svg");
        assert_eq!(
            preview_src(&def(None, "table")),
            "/static/previews/table.svg"
        );
        assert_eq!(
            preview_src(&def(None, "map")),
            "/static/previews/generic.svg"
        );
    }
}
