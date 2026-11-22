//! Plugin code widgets: the anchor the browser fills with the element the
//! `Widget` DocType's stored module defines.
//!
//! # What may be loaded
//!
//! The module and stylesheet URLs come from the `Widget` row, so the Desk
//! treats them as data: only a same-origin `/api/widgets/...` or `/plugins/...`
//! asset is loaded ([`local_asset`]). An absolute `http(s)://` URL is refused,
//! a `//host` protocol-relative URL is refused, and so is any `..` segment. The
//! row itself is `System Manager`-only (`ferris-schema/src/doctypes/views.rs`,
//! `widget_meta`), so a non-admin cannot write the URL in the first place; this
//! is the second gate, on the reader's side (WEB-02).
//!
//! The remaining limit is the injection mechanism: `loader::ensure_script`
//! creates a plain `<script type="module" src>` element and cannot attach a
//! `nonce` or an `integrity` hash, so those are not available here. The CSP
//! the server sends (`script-src 'self' 'nonce-…'`) is what bounds the module
//! to this origin; a same-origin URL is therefore trusted to the same degree
//! as the Desk's own bundle.

use leptos::prelude::*;

#[cfg(target_arch = "wasm32")]
use crate::ext::AppUi;
use crate::ext::UiCtx;

/// A widget instance resolved against the feed's catalog: everything the code
/// body needs to load the stored module, adopt its CSS and mount its element.
/// Built once by [`super::host::WidgetHost`] so the view and the mount effect
/// share one shape. `owner`/`component` are read by the wasm mount only; native
/// (SSR/tests) renders the body from the rest.
#[derive(Clone)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(super) struct ResolvedWidget {
    /// Publishing app: owns the shared stylesheet the widget inherits.
    pub(super) owner: String,
    /// The reference as declared (`owner.name` or bare `name`), mirrored on
    /// the element as `data-widget`.
    pub(super) reference: String,
    /// Instance id, unique within the dashboard (kept on the anchor for the
    /// desk's own hooks).
    pub(super) instance_id: String,
    /// Host-resolved title, exposed to the plugin as `data-label`; the host
    /// already painted it above the body, so this is informational.
    pub(super) label: String,
    /// Definition kind (`stat`/`chart`/`table`): the card's `data-type`.
    pub(super) kind: String,
    /// Definition component: the custom element the card mounts.
    pub(super) component: String,
    /// URL of the module the host serves from the `Widget` DocType. Present by
    /// construction: [`super::host::WidgetHost`] renders
    /// [`super::placeholder::missing_code_widget`] otherwise.
    pub(super) code_url: Option<String>,
    /// URL of the widget's own CSS, when its row stores one.
    pub(super) style_url: Option<String>,
    /// Instance parameter values, sent to the element as `data-params`.
    pub(super) params: Vec<(String, String)>,
}

/// One code widget body: an anchor the client fills with the live custom
/// element after loading the module stored in the `Widget` DocType.
///
/// The host paints the card and the heading (title/subtitle) above this
/// component; the widget contributes **only** the body. That is the split the
/// style contract needs: padding, background, border and radius belong to the
/// host's tile (`--ferris-tile-*`), so the app never declares them and the
/// host's heading never competes with a heading the app painted itself. The
/// definition's label still travels to the element as `data-label` for an app
/// that wants it inside its own content.
#[component]
pub(super) fn CodeWidget(widget: ResolvedWidget) -> impl IntoView {
    let ctx = expect_context::<UiCtx>();
    let anchor: NodeRef<leptos::html::Div> = NodeRef::new();
    let mounted = widget.clone();
    let params_json = serde_json::to_string(&widget.params).unwrap_or_else(|_| "[]".into());

    // Effects run post-hydration, so SSR emits only the empty body — the
    // element is the client's to add.
    Effect::new(move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            // The app manifest still loads the shared runtime (`core.js`,
            // app-wide CSS); the widget's own module is per-row and injected
            // by `mount_widget`.
            let apps = ctx.apps.get();
            crate::ext::loader::ensure_loaded(&apps);
            mount_widget(anchor.get(), &mounted, &apps);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (anchor, ctx, &mounted);
    });

    view! {
        <div class="min-w-0">
            <div
                node_ref=anchor
                data-widget=widget.reference.clone()
                data-instance=widget.instance_id.clone()
                data-type=widget.kind.clone()
                data-params=params_json
            ></div>
        </div>
    }
}

/// Whether `url` is a widget asset the Desk may load: a same-origin path under
/// `/api/widgets/` (the canonical per-row endpoint) or `/plugins/` (the app's
/// own asset route).
///
/// A widget row is data: an administrator can edit it, a migration can seed it,
/// and a bug can store an absolute URL in it. The rule is therefore a prefix
/// allowlist, not a blocklist: anything with a scheme (`http:`, `https:`,
/// `javascript:`, `data:`), a protocol-relative `//host` form, a backslash or a
/// `..`/`.` segment is refused, and so is every path outside the two prefixes.
/// The script is injected into the Desk's `document.head`, where a shadow root
/// is no isolation at all (WEB-02).
#[cfg(any(target_arch = "wasm32", test))]
fn local_asset(url: &str) -> Option<&str> {
    let url = url.trim();
    if url.is_empty()
        || url.contains("://")
        || url.contains('\\')
        || url.chars().any(char::is_control)
        || !url.starts_with('/')
        || url.starts_with("//")
        || url
            .split('/')
            .any(|segment| segment == ".." || segment == ".")
    {
        return None;
    }
    let allowed = url.starts_with("/api/widgets/") || url.starts_with("/plugins/");
    allowed.then_some(url)
}

/// Mount one dashboard widget into its anchor's shadow root: inject the module
/// the host serves from the `Widget` DocType, then swap in the custom element
/// the moment it is defined. Re-running (a feed refresh) is idempotent: the
/// mount container is cleared before the element is appended, and the styles
/// are re-adopted (never duplicated). F6 keeps the markup and CSS inside the
/// shadow tree, out of the Desk's global styles.
#[cfg(target_arch = "wasm32")]
fn mount_widget(anchor: Option<web_sys::HtmlDivElement>, widget: &ResolvedWidget, apps: &[AppUi]) {
    let Some(anchor) = anchor else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(shadow) = crate::ext::loader::shadow_root(&anchor) else {
        return;
    };
    let Some(container) = crate::ext::loader::mount_container(&shadow) else {
        return;
    };

    let registry = window.custom_elements();
    let tag = crate::ext::component_tag(&widget.component);
    // A URL the row should not carry is not loaded at all: the anchor keeps the
    // placeholder the host rendered, and nothing enters `document.head`.
    let Some(code_url) = widget.code_url.as_deref().and_then(local_asset) else {
        return;
    };

    // The module is stored in the `Widget` DocType and served by the host at
    // its original asset path (relative imports keep resolving) or the
    // canonical `/api/widgets/...` fallback. Injection is deduplicated.
    crate::ext::loader::ensure_script(code_url);

    // The publishing app's shared stylesheet plus the widget's own, scoped to
    // this widget's shadow tree.
    let mut urls = crate::ext::loader::stylesheet_urls_for_owners(apps, &[widget.owner.as_str()]);
    if let Some(style) = widget.style_url.as_deref().and_then(local_asset)
        && !urls.iter().any(|url| url == style)
    {
        urls.push(style.to_string());
    }
    crate::ext::loader::adopt(&shadow, &urls);

    let widget_ref = widget.reference.clone();
    let component = widget.component.clone();
    let label = widget.label.clone();
    let params_json = serde_json::to_string(&widget.params).unwrap_or_else(|_| "[]".into());

    crate::api::drive(async move {
        // Resolves at once when the element exists; otherwise when the
        // injected module finishes. A widget that never defines the component
        // simply keeps its placeholder.
        match registry.when_defined(&tag) {
            Ok(defined_promise) => {
                let _ = wasm_bindgen_futures::JsFuture::from(defined_promise).await;
            }
            Err(_) => return,
        }
        while let Some(child) = container.first_child() {
            let _ = container.remove_child(&child);
        }
        if let Ok(el) = document.create_element(&tag) {
            let _ = el.set_attribute("data-widget", &widget_ref);
            let _ = el.set_attribute("data-component", &component);
            let _ = el.set_attribute("data-label", &label);
            let _ = el.set_attribute("data-params", &params_json);
            let _ = container.append_child(el.as_ref());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::local_asset;

    /// WEB-02: only the two same-origin prefixes are loadable, so a `Widget`
    /// row cannot point the Desk's `document.head` at another origin.
    #[test]
    fn only_same_origin_widget_assets_are_accepted() {
        assert_eq!(
            local_asset("/api/widgets/app/kpi/script.js"),
            Some("/api/widgets/app/kpi/script.js")
        );
        assert_eq!(
            local_asset("/plugins/app/asset/ui/widgets/kpi.js?v=1"),
            Some("/plugins/app/asset/ui/widgets/kpi.js?v=1")
        );
        // Absolute, protocol-relative and scheme-bearing URLs are refused.
        assert_eq!(local_asset("https://evil.test/x.js"), None);
        assert_eq!(local_asset("http://evil.test/x.js"), None);
        assert_eq!(local_asset("//evil.test/x.js"), None);
        assert_eq!(local_asset("javascript:alert(1)"), None);
        assert_eq!(local_asset("data:text/javascript,alert(1)"), None);
        // Paths outside the two prefixes, and traversal out of them.
        assert_eq!(local_asset("/api/resource/User/x"), None);
        assert_eq!(local_asset("/plugins/../../etc/passwd"), None);
        assert_eq!(local_asset("/plugins/app/./x.js"), None);
        assert_eq!(local_asset("plugins/app/x.js"), None);
        assert_eq!(local_asset(""), None);
    }
}
