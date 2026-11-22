//! Plugin asset loader (Fase 1, scoped in F6): before the Desk mounts a
//! plugin's custom elements it must load the plugin's UI assets — the scripts
//! that `define()` those elements and the stylesheets that frame them.
//!
//! Every app declares an `assets` manifest in its `describe()` (list of path
//! strings). The host serves each path through the plugin's `ui-asset` export
//! at `/plugins/{owner}/asset/{path}`, with a content type guessed from the
//! extension.
//!
//! Scripts still go into `document.head` once per page load. **Stylesheets do
//! not**: F6 mounts plugin elements into an `open` shadow root, so a global
//! `<link>` would leak the plugin's CSS into the Desk. Instead each stylesheet
//! is fetched once and its `CSSStyleSheet` is adopted (shared by reference)
//! into the shadow roots that want it; a browser without constructable
//! stylesheets gets the same CSS injected as a `<style>` per root.
//!
//! Injection is best-effort on purpose: assets are also loaded when a view
//! mounts an extension (`UiSlot`), and a second attempt for an already-loaded
//! URL is a no-op, so a plugin whose script tag was dropped by the DOM still
//! gets its elements defined as soon as any slot needs them.

use super::AppUi;

/// `/plugins/{owner}/asset/{path}` — the URL the host serves an app's UI asset
/// at (relayed from the app's `ui-asset` export).
pub fn asset_url(owner: &str, path: &str) -> String {
    format!("/plugins/{owner}/asset/{path}")
}

/// The lowercase extension of an asset path (`"ui/app.js"` → `"js"`). Empty
/// when the path has no extension, e.g. an unusual manifest entry.
pub fn asset_extension(path: &str) -> &str {
    let tail = path.rsplit('/').next().unwrap_or(path);
    match tail.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext_key(ext),
        _ => "",
    }
}

fn ext_key(ext: &str) -> &str {
    // Match over the lowered form only for the closed set of known script
    // extensions; anything else keeps its original casing as-is.
    if matches!(ext.to_ascii_lowercase().as_str(), "js" | "mjs" | "cjs") {
        return "js";
    }
    if ext.eq_ignore_ascii_case("css") {
        return "css";
    }
    ext
}

/// Whether the asset is a script the loader injects as a module.
pub fn is_script(path: &str) -> bool {
    asset_extension(path) == "js"
}

/// Whether the asset is a stylesheet (F6 scopes these to shadow roots instead
/// of linking them globally).
pub fn is_stylesheet(path: &str) -> bool {
    asset_extension(path) == "css"
}

/// The deduplicated asset URLs for every app's declared manifest, in
/// declaration order (first app, then paths). The Desk loads each once.
pub fn asset_urls(apps: &[AppUi]) -> Vec<String> {
    collect_assets(apps.iter())
}

/// The stylesheet (`*.css`) URLs declared by `owners`, in declaration order and
/// deduplicated. F6 scopes plugin CSS: instead of linking these globally, the
/// loader fetches each once and shares the sheet with the shadow roots that ask
/// for it. A slot anchor adopts the CSS of every app that declares into it; a
/// widget or website page adopts the CSS of its publishing app.
pub fn stylesheet_urls_for_owners(apps: &[AppUi], owners: &[&str]) -> Vec<String> {
    collect_assets(
        apps.iter()
            .filter(|app| owners.contains(&app.owner.as_str())),
    )
    .into_iter()
    .filter(|url| is_stylesheet(url))
    .collect()
}

/// The deduplicated asset URLs of `apps`, in declaration order.
fn collect_assets<'a>(apps: impl Iterator<Item = &'a AppUi>) -> Vec<String> {
    let mut seen = Vec::new();
    let mut urls = Vec::new();
    for app in apps {
        for path in &app.assets {
            let url = asset_url(&app.owner, path);
            if !seen.contains(&url) {
                seen.push(url.clone());
                urls.push(url);
            }
        }
    }
    urls
}

/// Attach an `open` shadow root to `host`, or return the one already attached.
/// Open mode is deliberate: the desk's tooling (tests, devtools, the console)
/// can still reach the plugin's tree, while plugin CSS no longer leaks into the
/// host and host CSS no longer styles the plugin's elements — only inherited
/// properties (the theme's custom properties among them) cross the boundary.
#[cfg(target_arch = "wasm32")]
pub fn shadow_root(host: &web_sys::Element) -> Option<web_sys::ShadowRoot> {
    if let Some(root) = host.shadow_root() {
        return Some(root);
    }
    host.attach_shadow(&web_sys::ShadowRootInit::new(web_sys::ShadowRootMode::Open))
        .ok()
}

/// The child of `root` the plugin elements are mounted into. Keeping them in a
/// container (rather than directly under the shadow root) leaves room for the
/// `<style>` fallback elements [`adopt`] may inject, so refreshing a mount
/// never wipes the styles.
#[cfg(target_arch = "wasm32")]
pub fn mount_container(root: &web_sys::ShadowRoot) -> Option<web_sys::Element> {
    use wasm_bindgen::JsCast;
    let fragment: &web_sys::DocumentFragment = root.unchecked_ref();
    if let Ok(Some(existing)) = fragment.query_selector("[data-ferris-mount]") {
        return Some(existing);
    }
    let container = web_sys::window()?.document()?.create_element("div").ok()?;
    let _ = container.set_attribute("data-ferris-mount", "");
    let _ = root.append_child(container.as_ref());
    Some(container)
}

/// Adopt `urls`' stylesheets into `root`, fetching the ones not loaded yet and
/// sharing each fetched sheet across every root that wants it ("fetch once,
/// adopt everywhere"). Idempotent: the root's adopted sheets are rebuilt from
/// `urls`, so re-mounts (a manifest refresh, a doctype change) do not
/// accumulate duplicates.
#[cfg(target_arch = "wasm32")]
pub fn adopt(root: &web_sys::ShadowRoot, urls: &[String]) {
    styles::adopt(root, urls);
}

/// Inject every not-yet-loaded **script** of the current apps into
/// `document.head` as `<script type="module">`, and start fetching every
/// stylesheet once. Deduplicated by URL across calls, so hot manifests and
/// repeated slot mounts never double-load anything. Stylesheets are no longer
/// injected globally (F6): [`adopt`] shares the fetched sheet across the shadow
/// roots that ask for it.
#[cfg(target_arch = "wasm32")]
pub fn ensure_loaded(apps: &[AppUi]) {
    for url in asset_urls(apps) {
        if is_script(&url) {
            ensure_script(&url);
        }
    }

    // Fetch every declared stylesheet once; `adopt` shares it afterwards.
    for url in collect_assets(apps.iter()) {
        if is_stylesheet(&url) {
            styles::ensure_fetched(&url);
        }
    }
}

/// Inject one module script into `document.head` once. The widget catalog uses
/// this for the code stored in the `Widget` DocType, which is not part of an
/// app's manifest: a widget's module loads the first time a dashboard mounts it
/// and is deduplicated by URL across re-mounts.
#[cfg(target_arch = "wasm32")]
pub fn ensure_script(url: &str) {
    use wasm_bindgen::JsCast;

    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(head) = document.head() else {
        return;
    };

    let mut loaded = SCRIPTS
        .get_or_init(|| std::sync::Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if loaded.iter().any(|seen| seen == url) {
        return;
    }
    if let Ok(el) = document.create_element("script") {
        if let Ok(script) = el.dyn_into::<web_sys::HtmlScriptElement>() {
            script.set_type("module");
            script.set_src(url);
            let _ = head.append_child(script.as_ref());
        }
    }
    // Remember the URL whatever happened (element/head missing): a second
    // attempt on the next mount would only redo the same work.
    loaded.push(url.to_string());
}

/// URLs of the scripts already injected on this page load.
#[cfg(target_arch = "wasm32")]
static SCRIPTS: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();

/// F6 stylesheet cache: fetch each app stylesheet once and adopt the resulting
/// constructable `CSSStyleSheet` (or, on a browser without them, inject its text
/// as a `<style>`) into every shadow root that declared the URL.
#[cfg(target_arch = "wasm32")]
mod styles {
    use std::collections::{HashMap, HashSet};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    use wasm_bindgen::{JsCast, JsValue};
    use web_sys::{CssStyleSheet, ShadowRoot};

    /// One loaded app stylesheet.
    enum Sheet {
        /// A constructable stylesheet, adopted (by reference) into every root
        /// that wants it: one network fetch, N shadow trees, no duplication.
        Adopted(CssStyleSheet),
        /// Fallback for a browser without constructable stylesheets: the raw
        /// text, injected as one `<style>` per shadow root that wants it.
        Inline(String),
    }

    /// A shadow root and the stylesheet URLs it wants. Remembered so a sheet
    /// that arrives after the root was created is still adopted into it.
    #[derive(Clone)]
    struct Wish {
        root: ShadowRoot,
        urls: Vec<String>,
    }

    #[derive(Default)]
    struct Cache {
        ready: HashMap<String, Sheet>,
        /// URLs with a fetch in flight (or already attempted): never fetched
        /// twice, a failing fetch is not retried per mount.
        requested: HashSet<String>,
        wishes: Vec<Wish>,
    }

    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

    fn lock() -> MutexGuard<'static, Cache> {
        CACHE
            .get_or_init(|| Mutex::new(Cache::default()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Adopt `urls`' stylesheets into `root`, fetching the missing ones.
    pub fn adopt(root: &ShadowRoot, urls: &[String]) {
        {
            let mut cache = lock();
            match cache
                .wishes
                .iter_mut()
                .find(|wish| same_root(&wish.root, root))
            {
                Some(wish) => wish.urls = urls.to_vec(),
                None => cache.wishes.push(Wish {
                    root: root.clone(),
                    urls: urls.to_vec(),
                }),
            }
        }
        apply(&Wish {
            root: root.clone(),
            urls: urls.to_vec(),
        });
        for url in urls {
            ensure_fetched(url);
        }
    }

    /// Start fetching `url` once. When it lands it is stored and re-applied to
    /// every root that wanted it.
    pub fn ensure_fetched(url: &str) {
        {
            let mut cache = lock();
            if !cache.requested.insert(url.to_string()) {
                return;
            }
        }
        let url = url.to_string();
        crate::api::drive(async move {
            // A stylesheet that will not load leaves the app unstyled; retrying
            // would only repeat the same failing fetch.
            let Ok(css) = crate::api::client::api_get_raw(&url).await else {
                return;
            };
            let sheet = match CssStyleSheet::new() {
                Ok(sheet) if sheet.replace_sync(&css).is_ok() => Sheet::Adopted(sheet),
                _ => Sheet::Inline(css),
            };
            let wishes: Vec<Wish> = {
                let mut cache = lock();
                cache.ready.insert(url.clone(), sheet);
                cache.wishes.clone()
            };
            for wish in wishes {
                if wish.urls.iter().any(|wanted| wanted == &url) {
                    apply(&wish);
                }
            }
        });
    }

    /// Re-point `wish`'s root at exactly its wanted sheets: constructable
    /// sheets replace `adoptedStyleSheets`, the inline fallback keeps one
    /// `<style>` per URL.
    fn apply(wish: &Wish) {
        let (adopted, inline): (Vec<JsValue>, Vec<(String, String)>) = {
            let cache = lock();
            let mut adopted = Vec::new();
            let mut inline = Vec::new();
            for url in &wish.urls {
                match cache.ready.get(url) {
                    Some(Sheet::Adopted(sheet)) => adopted.push(JsValue::from(sheet.clone())),
                    Some(Sheet::Inline(css)) => inline.push((url.clone(), css.clone())),
                    None => {}
                }
            }
            (adopted, inline)
        };
        let array = js_sys::Array::new();
        for sheet in &adopted {
            array.push(sheet);
        }
        wish.root.set_adopted_style_sheets(&JsValue::from(array));
        for (url, css) in inline {
            ensure_inline(&wish.root, &url, &css);
        }
    }

    /// The `<style>` fallback for one URL, appended once per shadow root.
    fn ensure_inline(root: &ShadowRoot, url: &str, css: &str) {
        let fragment: &web_sys::DocumentFragment = root.unchecked_ref();
        let selector = format!("style[data-ferris-sheet={url:?}]");
        if fragment.query_selector(&selector).ok().flatten().is_some() {
            return;
        }
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        if let Ok(style) = document.create_element("style") {
            let _ = style.set_attribute("data-ferris-sheet", url);
            style.set_text_content(Some(css));
            let _ = root.append_child(style.as_ref());
        }
    }

    /// Whether two handles point at the same shadow root (`Object.is`).
    fn same_root(a: &ShadowRoot, b: &ShadowRoot) -> bool {
        js_sys::Object::is(a.as_ref(), b.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_schema::contract::AppUi;

    fn app(owner: &str, assets: &[&str]) -> AppUi {
        AppUi {
            owner: owner.to_string(),
            assets: assets.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn asset_urls_are_anchored_at_the_plugin_route() {
        assert_eq!(
            asset_url("test", "ui/task-actions.js"),
            "/plugins/test/asset/ui/task-actions.js"
        );
    }

    #[test]
    fn asset_extensions_detect_scripts_and_stylesheets() {
        assert!(is_script("ui/app.js"));
        assert!(is_script("ui/app.mjs"));
        assert!(is_stylesheet("ui/app.css"));
        assert!(!is_script("ui/app.css"));
        assert!(!is_stylesheet("ui/app.js"));
        // Unknown or missing extensions are neither — the loader skips them.
        assert_eq!(asset_extension("img/logo.png"), "png");
        assert_eq!(asset_extension("README"), "");
    }

    #[test]
    fn asset_urls_dedupe_and_keep_declaration_order() {
        let apps = vec![
            app("app-a", &["ui/a.js", "ui/shared.css"]),
            app("app-b", &["ui/b.js"]),
            app("app-a", &["ui/a.js", "ui/new.js"]),
        ];
        assert_eq!(
            asset_urls(&apps),
            vec![
                "/plugins/app-a/asset/ui/a.js",
                "/plugins/app-a/asset/ui/shared.css",
                "/plugins/app-b/asset/ui/b.js",
                "/plugins/app-a/asset/ui/new.js",
            ]
        );
    }

    #[test]
    fn an_app_without_assets_declares_nothing_to_load() {
        assert!(asset_urls(&[app("app-a", &[])]).is_empty());
    }

    /// F6: only the owners that declare into a mount contribute stylesheets, and
    /// the script half of a manifest never leaks into the CSS set.
    #[test]
    fn stylesheet_urls_are_scoped_to_the_owners_and_only_css() {
        let apps = vec![
            app("app-a", &["ui/a.js", "ui/a.css"]),
            app("app-b", &["ui/b.css"]),
            app("app-c", &["ui/c.css", "ui/c.css"]),
        ];

        assert_eq!(
            stylesheet_urls_for_owners(&apps, &["app-a", "app-c"]),
            vec![
                "/plugins/app-a/asset/ui/a.css",
                "/plugins/app-c/asset/ui/c.css",
            ],
        );
        // A slot with extensions from one app adopts only that app's CSS; an
        // unknown owner contributes nothing.
        assert_eq!(
            stylesheet_urls_for_owners(&apps, &["app-b"]),
            vec!["/plugins/app-b/asset/ui/b.css"]
        );
        assert!(stylesheet_urls_for_owners(&apps, &["nope"]).is_empty());
        assert!(stylesheet_urls_for_owners(&apps, &[]).is_empty());
    }
}
