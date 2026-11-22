//! Inline SVG helpers shared by the widget previews and the Desk grid.
//!
//! Apps may declare an icon as an inline SVG document instead of an icon name.
//! Two things have to be true for that to be safe and to look right:
//!
//! - the markup is **sanitized** before it reaches the document (an app must not
//!   be able to turn an icon into script execution), and it is mounted inside a
//!   **shadow root** so the SVG's own `<style>` cannot leak onto the Desk;
//! - it is **inline**, not an `<img>`, so `currentColor` and the runtime theme's
//!   custom properties reach it (an SVG loaded as an image is an isolated
//!   document: it cannot read `var(--primary)` and cannot inherit `color`).
//!
//! SSR cannot sanitize with a DOM, so the server (and the first hydration pass)
//! render a data-URI `<img>` instead — an SVG in image mode runs no scripts and
//! loads no external resources, so it is a safe fallback. A post-hydration effect
//! then replaces it with the sanitized, inline version. If the sanitizer refuses
//! the markup, the `<img>` simply stays.
//!
//! [`sanitize_svg`] was originally private to the dashboard preview module; it
//! lives here now because the Desk grid needs exactly the same policy. Two
//! sanitizers with two rule sets would be a security bug waiting for the first
//! one to drift.

/// Whether `text` is an SVG document rather than an icon name.
///
/// The check is deliberately cheap: the sanitizer is what actually decides, and
/// a string that merely starts with `<svg` but is not one is refused there. This
/// only routes the two shapes (`"BookOpen"` vs `"<svg …>"`) to different
/// renderers.
pub fn looks_like_svg(text: &str) -> bool {
    text.trim_start().starts_with("<svg")
}

/// A data-URI `src` for SVG markup, used as the SSR/hydration fallback.
///
/// Everything outside RFC 3986's unreserved set is percent-encoded, including
/// `#`, which would otherwise start a fragment and truncate the document inline.
pub fn data_uri(markup: &str) -> String {
    let mut out = String::with_capacity(markup.len() + 32);
    out.push_str("data:image/svg+xml;charset=utf-8,");
    for byte in markup.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Sanitize an app-authored SVG document, returning the markup to inline or
/// `None` when the text is not an SVG at all.
///
/// The rules are deliberately narrow — SVG, not arbitrary HTML:
///
/// - the single root element must be `<svg>` (an HTML document, a fragment with
///   siblings, or an `<html>` root is refused);
/// - `<script>` and `<foreignObject>` (and their subtrees) are removed;
/// - every `on*` event attribute is removed;
/// - `href`/`xlink:href` are removed unless they are a same-document
///   `#fragment`, so `javascript:` and off-origin references cannot survive.
///   On `<use>` even the fragment is dropped: that reference resolves against
///   the Desk document, not the shadow tree, and cloning an arbitrary element
///   into the icon is exactly the kind of surprise this policy excludes. `src`
///   is always removed (an inline icon needs no external resource);
/// - any attribute whose `url(...)` names another document (`fill="url(http…)"`,
///   `filter="url(https…)"`, `style="mask:url(…)"`) is removed: an icon may
///   point at its own `#defs`, never at a remote file;
/// - `<style>` is kept (it is how an SVG sets its own token fallbacks) unless
///   its CSS names another document (`url(http…)`) or imports one (`@import`),
///   which an inline icon never needs — then the whole element is refused;
/// - `<link>` is always removed (it exists only to load an external sheet).
///
/// The result is fresh **markup**, not a live node: the caller hands it to a
/// shadow root, and the scratch document it was parsed in never sees it.
#[cfg(target_arch = "wasm32")]
pub fn sanitize_svg(text: &str) -> Option<String> {
    let document = web_sys::window()?.document()?;
    // A detached `<div>` parses the string with the HTML parser (which knows
    // SVG-in-HTML); nothing it holds is ever connected to the document.
    let scratch = document.create_element("div").ok()?;
    scratch.set_inner_html(text);

    let root = scratch.first_element_child()?;
    if !root.tag_name().eq_ignore_ascii_case("svg") {
        return None;
    }

    let mut stack: Vec<web_sys::Element> = vec![root.clone()];
    while let Some(element) = stack.pop() {
        let tag = element.tag_name().to_ascii_lowercase();
        if matches!(tag.as_str(), "script" | "foreignobject" | "link")
        // A `<style>` whose CSS can fetch (a `url(...)`) stops being a
        // paint-only rule: an inline icon has no reason to load a resource, so
        // the whole element goes rather than a half-guessed rewrite.
        || (tag == "style" && style_can_fetch(&element))
        {
            element.remove();
            continue;
        }
        sanitize_attributes(&element, &tag);
        let mut child = element.first_element_child();
        while let Some(node) = child {
            // Read the next sibling before the loop body can remove this one.
            child = node.next_element_sibling();
            stack.push(node);
        }
    }
    Some(root.outer_html())
}

/// Whether a `<style>` element's CSS text names an external resource.
#[cfg(target_arch = "wasm32")]
fn style_can_fetch(element: &web_sys::Element) -> bool {
    element.text_content().is_some_and(|css| {
        let css = css.to_ascii_lowercase();
        !only_local_urls(&css) || css.contains("@import")
    })
}

/// Whether every `url(...)` in `value` is a same-document reference.
///
/// Presentation attributes (`fill`, `stroke`, `filter`, `mask`, `clip-path`,
/// ...) and `style` can name a resource just like `<style>` can. An icon has a
/// reason to point at its own defs and none to fetch anything, so a value that
/// references an external document is dropped whole — half-rewriting a paint
/// server is how a sanitizer grows bugs.
#[cfg(any(target_arch = "wasm32", test))]
fn only_local_urls(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(start) = rest.find("url(") {
        rest = &rest[start + 4..];
        if !rest.trim_start().starts_with('#') {
            return false;
        }
    }
    true
}

/// Strip the attributes [`sanitize_svg`] forbids from one element.
#[cfg(target_arch = "wasm32")]
fn sanitize_attributes(element: &web_sys::Element, tag: &str) {
    let attrs = element.attributes();
    // `length`/`item` are `NamedNodeMap`'s core methods; the iteration helpers
    // are behind an extra web-sys feature this crate does not enable.
    let mut names: Vec<String> = Vec::with_capacity(attrs.length() as usize);
    for i in 0..attrs.length() {
        if let Some(attr) = attrs.item(i) {
            names.push(attr.name());
        }
    }
    for name in names {
        let lower = name.to_ascii_lowercase();
        let value = element.get_attribute(&name).unwrap_or_default();
        let drop = lower.starts_with("on")
            || lower == "src"
            || (matches!(lower.as_str(), "href" | "xlink:href") && !value.starts_with('#'))
            // `<use>` is only a same-document reference; anything else is a
            // fetch this icon has no reason to perform.
            || (tag == "use" && matches!(lower.as_str(), "href" | "xlink:href"))
            // A paint server or a `style` must not name another document.
            || !only_local_urls(&value);
        if drop {
            let _ = element.remove_attribute(&name);
        }
    }
}

/// Sanitize `markup` and mount it inline inside `host`'s shadow root.
///
/// The shadow root is both the isolation (an SVG `<style>` cannot leak onto the
/// Desk) and the inheritance channel (`currentColor` and the theme's custom
/// properties cross the boundary). The wrapper makes the SVG fill the box the
/// caller sized, since inline SVGs carry no `width`/`height` of their own.
///
/// A no-op when the markup is refused, so the caller's `<img>` fallback stays.
#[cfg(target_arch = "wasm32")]
pub fn mount_inline(host: &web_sys::Element, markup: &str) {
    let Some(clean) = sanitize_svg(markup) else {
        return;
    };
    let Some(root) = crate::ext::loader::shadow_root(host) else {
        return;
    };
    root.set_inner_html(&format!(
        "<style>\
           :host {{ display:block; width:100%; height:100%; }}\
           svg {{ display:block; width:100%; height:100%; }}\
         </style>{clean}"
    ));
}

/// Non-wasm stub: no DOM exists to mount into.
#[cfg(not(target_arch = "wasm32"))]
pub fn mount_inline<H>(_host: &H, _markup: &str) {}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn only_markup_starting_with_an_svg_root_is_treated_as_svg() {
        assert!(looks_like_svg(
            "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"
        ));
        assert!(looks_like_svg("  \n<svg></svg>"));
        assert!(!looks_like_svg("BookOpen"));
        assert!(!looks_like_svg(""));
        assert!(!looks_like_svg("<div></div>"));
    }

    #[test]
    fn only_same_document_paint_and_style_urls_are_kept() {
        assert!(only_local_urls("none"));
        assert!(only_local_urls("url(#brand-gradient)"));
        assert!(only_local_urls("fill: url( #shadow ); stroke: none"));
        assert!(!only_local_urls("url(https://example.test/pattern.svg)"));
        assert!(!only_local_urls(
            "fill: url(#ok); mask: url('https://example.test/x')"
        ));
        assert!(!only_local_urls("filter: url(data:image/svg+xml,x)"));
    }

    #[test]
    fn the_data_uri_percent_encodes_everything_but_unreserved_characters() {
        let uri = data_uri("<svg fill=\"#fff\"></svg>");
        assert!(uri.starts_with("data:image/svg+xml;charset=utf-8,"));
        assert!(uri.contains("%3Csvg"));
        assert!(
            !uri.contains('#'),
            "a raw # would truncate the URI fragment"
        );
        assert!(uri.contains("fill%3D%22%23fff%22"));
    }
}
