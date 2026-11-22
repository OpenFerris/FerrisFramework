//! Lazy loader for the vendored third-party browser libraries.
//!
//! The Desk ships Leaflet, JsBarcode, Quill and marked inside the binary
//! ([`crate::vendor_assets`]) and serves them from `/static/vendor/*`; this
//! module injects their `<link>`/`<script>` tags into `document.head` the first
//! time a widget asks for them and runs that widget's callback once **every**
//! asset of the bundle has settled (loaded *or* failed, always exactly once):
//!
//! - waiting for the styles too means a map never initializes before its CSS
//!   exists, and
//! - a failed load still gets control: the callback runs, finds the global
//!   missing through [`global`], logs loudly, and the widget keeps its plain
//!   fallback on screen instead of hanging on an spinner that never resolves.
//!
//! Why lazy: the four bundles total ~600 KB. A Desk session that never opens a
//! form with a `Geolocation`, `Barcode`, `Text Editor` or `Markdown Editor`
//! field must not pay for them — the same reasoning as the plugin loader
//! ([`super::loader`]), one level down.
//!
//! Deduplication is by URL across calls (a hot reload or a second widget of
//! the same kind injects nothing twice) and per bundle for completion: the
//! first caller of a bundle starts the in-flight count, later callers only
//! queue their callback. Bundles must therefore not share an asset URL —
//! pinned by `disjoint_bundles_do_not_share_urls`.
//!
//! Native (`ssr`/typecheck) twin: a no-op. SSR renders each widget's fallback
//! markup and effects never run there; the shim exists only so the same view
//! compiles for both targets, exactly like `signature.rs`.

/// One vendored bundle a widget can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Vendor {
    Leaflet,
    JsBarcode,
    Quill,
    Marked,
}

impl Vendor {
    /// Stylesheets (paths under `/static/vendor/`), fetched in parallel.
    pub const fn styles(self) -> &'static [&'static str] {
        match self {
            Vendor::Leaflet => &["leaflet/leaflet.css", "leaflet-draw/leaflet.draw.css"],
            Vendor::JsBarcode => &[],
            Vendor::Quill => &["quill/quill.core.css", "quill/quill.snow.css"],
            Vendor::Marked => &[],
        }
    }

    /// Scripts (paths under `/static/vendor/`), executed in this order —
    /// `leaflet-draw` extends the `L` global `leaflet` defines, so insertion
    /// order is load-bearing (`async = false` in the injector).
    pub const fn scripts(self) -> &'static [&'static str] {
        match self {
            Vendor::Leaflet => &["leaflet/leaflet.js", "leaflet-draw/leaflet.draw.js"],
            Vendor::JsBarcode => &["jsbarcode/JsBarcode.all.min.js"],
            Vendor::Quill => &["quill/quill.js"],
            Vendor::Marked => &["marked/marked.min.js"],
        }
    }

    /// The `window` global the bundle defines. Widgets must check it via
    /// [`global`] before calling into it: the `ensure` callback fires on
    /// failure too, so "callback ran" never means "library is there".
    pub const fn global(self) -> &'static str {
        match self {
            Vendor::Leaflet => "L",
            Vendor::JsBarcode => "JsBarcode",
            Vendor::Quill => "Quill",
            Vendor::Marked => "marked",
        }
    }
}

/// Run `ready` once `vendor`'s assets have all settled: immediately if the
/// bundle already ran, otherwise after the last in-flight asset of that
/// bundle (started by this or an earlier call) fires `load`/`error`.
///
/// Idempotent per URL and per bundle; safe to call from every value-change
/// effect (once loaded it is a synchronous call).
#[cfg(target_arch = "wasm32")]
pub fn ensure(vendor: Vendor, ready: impl FnOnce() + 'static) {
    browser::ensure(vendor, Box::new(ready))
}

/// Native twin: no browser to load into. Views compile against this so the
/// `ssr` feature does not need `cfg` at every call site; the callback never
/// runs, which is fine because SSR renders fallback markup (and effects do
/// not execute during SSR).
#[cfg(not(target_arch = "wasm32"))]
pub fn ensure(_vendor: Vendor, _ready: impl FnOnce() + 'static) {}

/// Whether `vendor`'s assets have already settled on this page — lets a
/// widget skip queueing when it just needs to know. Always `false` natively.
#[cfg(target_arch = "wasm32")]
pub fn is_ready(vendor: Vendor) -> bool {
    browser::is_ready(vendor)
}

/// Native twin of [`is_ready`].
#[cfg(not(target_arch = "wasm32"))]
pub fn is_ready(_vendor: Vendor) -> bool {
    false
}

/// The bundle's `window` global (`L`, `JsBarcode`, `Quill`, `marked`) —
/// `None` before it ran or when it failed to load, which is the signal a
/// widget's ready-callback uses to fall back instead of calling into nothing.
#[cfg(target_arch = "wasm32")]
pub fn global(vendor: Vendor) -> Option<wasm_bindgen::JsValue> {
    let value = js_sys::Reflect::get(
        &js_sys::global(),
        &wasm_bindgen::JsValue::from_str(vendor.global()),
    )
    .ok()?;
    if value.is_undefined() || value.is_null() {
        None
    } else {
        Some(value)
    }
}

/// Thin `js_sys::Reflect` helpers for calling into the vendored libraries'
/// globals from the form widgets. Every operation is fallible and
/// returns `None` instead of unwrapping: a missing property or method is a
/// logged fallback up the stack, never a panic on a request path.
#[cfg(target_arch = "wasm32")]
pub mod js {
    use wasm_bindgen::{JsCast, JsValue};

    /// `obj.key`
    pub fn get(obj: &JsValue, key: &str) -> Option<JsValue> {
        js_sys::Reflect::get(obj, &JsValue::from_str(key)).ok()
    }

    /// `obj.key(...args)` — `this` inside the call is `obj`, so Leaflet-style
    /// methods that read their receiver keep working.
    pub fn call(obj: &JsValue, method: &str, args: &[JsValue]) -> Option<JsValue> {
        let function = get(obj, method)?.dyn_into::<js_sys::Function>().ok()?;
        let argv = js_sys::Array::new();
        for arg in args {
            argv.push(arg);
        }
        function.apply(obj, &argv).ok()
    }

    /// `new Key(...args)` — for constructors exposed as globals (`Quill`).
    pub fn construct(ctor: &JsValue, args: &[JsValue]) -> Option<JsValue> {
        let function = ctor.clone().dyn_into::<js_sys::Function>().ok()?;
        let argv = js_sys::Array::new();
        for arg in args {
            argv.push(arg);
        }
        js_sys::Reflect::construct(&function, &argv).ok()
    }

    /// Log to the browser console — the loud half of "fall back instead of
    /// pretending".
    pub fn error(message: &str) {
        web_sys::console::error_1(&JsValue::from_str(message));
    }
}

#[cfg(target_arch = "wasm32")]
mod browser {
    use std::cell::RefCell;
    use std::collections::{HashMap, HashSet};

    use wasm_bindgen::{JsCast, closure::Closure};
    use web_sys::{HtmlLinkElement, HtmlScriptElement};

    use super::Vendor;

    /// Session-wide injection state, single-threaded (wasm): a `RefCell` in a
    /// thread-local, never held across an event dispatch — the `load`/`error`
    /// handlers re-enter `ensure`'s state only through short borrows.
    #[derive(Default)]
    struct State {
        /// Bundles whose assets have all settled.
        done: HashSet<Vendor>,
        /// Bundles whose assets were injected at least once.
        started: HashSet<Vendor>,
        /// Asset URLs already in the document (dedupe across calls/vendors).
        seen: HashSet<&'static str>,
        /// Assets still pending per in-flight bundle.
        inflight: HashMap<Vendor, usize>,
        /// Callbacks waiting for a bundle to settle.
        pending: HashMap<Vendor, Vec<Box<dyn FnOnce()>>>,
    }

    thread_local! {
        static STATE: RefCell<State> = RefCell::new(State::default());
    }

    pub(super) fn ensure(vendor: Vendor, ready: Box<dyn FnOnce()>) {
        if is_ready(vendor) {
            ready();
            return;
        }
        let inject = STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.pending.entry(vendor).or_default().push(ready);
            if state.started.insert(vendor) {
                state
                    .inflight
                    .insert(vendor, vendor.styles().len() + vendor.scripts().len());
                true
            } else {
                false
            }
        });
        if inject {
            inject_assets(vendor);
        }
    }

    pub(super) fn is_ready(vendor: Vendor) -> bool {
        STATE.with(|state| state.borrow().done.contains(&vendor))
    }

    /// Put the bundle's `<link>`s and `<script>`s in `document.head`.
    ///
    /// Scripts are inserted classic (the dist files are UMD globals) with
    /// `async = false`, which makes the browser execute them in insertion
    /// order while still downloading in parallel — `leaflet.draw` therefore
    /// always sees `L`. Every asset gets one-shot `load` + `error` listeners;
    /// both paths funnel into [`settle`], so a 404 completes the bundle rather
    /// than stranding its callbacks.
    fn inject_assets(vendor: Vendor) {
        let Some(document) = web_sys::window().and_then(|win| win.document()) else {
            fail(vendor, "vendor loader: no document");
            return;
        };
        let Some(head) = document.head() else {
            fail(vendor, "vendor loader: no document.head");
            return;
        };

        let assets = vendor
            .styles()
            .iter()
            .map(|url| (*url, true))
            .chain(vendor.scripts().iter().map(|url| (*url, false)));

        for (rel, is_style) in assets {
            let already_in_document = STATE.with(|state| {
                let mut state = state.borrow_mut();
                if state.seen.contains(rel) {
                    true
                } else {
                    state.seen.insert(rel);
                    false
                }
            });
            if already_in_document {
                // Someone else already put this URL in the document (the
                // bundles are disjoint today — pinned by test — so this is
                // pure defence): this bundle counted it in `inflight`, so the
                // counter must give it back even though we inject nothing.
                settle(vendor, 1);
                continue;
            }

            let url = format!("/static/vendor/{rel}");
            let Ok(element) = document.create_element(if is_style { "link" } else { "script" })
            else {
                settle(vendor, 1);
                continue;
            };

            if is_style {
                let Ok(link) = element.dyn_into::<HtmlLinkElement>() else {
                    settle(vendor, 1);
                    continue;
                };
                link.set_rel("stylesheet");
                link.set_href(&url);
                attach(&link, vendor, rel);
                let _ = head.append_child(&link);
            } else {
                let Ok(script) = element.dyn_into::<HtmlScriptElement>() else {
                    settle(vendor, 1);
                    continue;
                };
                script.set_src(&url);
                // Insertion order = execution order, downloads in parallel —
                // `leaflet.draw` therefore always sees the `L` global.
                script.set_async(false);
                attach(&script, vendor, rel);
                let _ = head.append_child(&script);
            }
        }
    }

    /// Nothing can load (no DOM): run the waiting callbacks anyway so widgets
    /// log and fall back instead of waiting on assets that will never come.
    fn fail(vendor: Vendor, message: &str) {
        web_sys::console::error_1(&wasm_bindgen::JsValue::from_str(message));
        STATE.with(|state| state.borrow_mut().inflight.remove(&vendor));
        run_pending(vendor);
    }

    /// `set_onload`/`set_onerror` are generated per concrete element type in
    /// `web-sys`, so this two-impl trait is what keeps [`attach`] generic.
    /// Property setters (unlike `add_event_listener`) return `()` — there is
    /// no failure path that could strand the in-flight counter.
    trait SettleHandlers {
        fn set_settle_handlers(
            &self,
            on_load: &wasm_bindgen::JsValue,
            on_error: &wasm_bindgen::JsValue,
        );
    }

    impl SettleHandlers for HtmlLinkElement {
        fn set_settle_handlers(
            &self,
            on_load: &wasm_bindgen::JsValue,
            on_error: &wasm_bindgen::JsValue,
        ) {
            self.set_onload(Some(on_load.unchecked_ref()));
            self.set_onerror(Some(on_error.unchecked_ref()));
        }
    }

    impl SettleHandlers for HtmlScriptElement {
        fn set_settle_handlers(
            &self,
            on_load: &wasm_bindgen::JsValue,
            on_error: &wasm_bindgen::JsValue,
        ) {
            self.set_onload(Some(on_load.unchecked_ref()));
            self.set_onerror(Some(on_error.unchecked_ref()));
        }
    }

    /// One-shot `load` + `error` handlers that count this asset down. Both are
    /// handed to JS with `once_into_js` (they run at most once — the two
    /// events are mutually exclusive) and kept alive by the element's
    /// reference to them, the same pattern as `signature.rs::restore`.
    fn attach<T: SettleHandlers>(element: &T, vendor: Vendor, rel: &'static str) {
        let on_load = Closure::once_into_js(move || settle(vendor, 1));
        let on_error = Closure::once_into_js(move || {
            web_sys::console::error_1(&wasm_bindgen::JsValue::from_str(&format!(
                "vendor asset failed to load: /static/vendor/{rel}"
            )));
            settle(vendor, 1);
        });
        element.set_settle_handlers(&on_load, &on_error);
    }

    /// One asset of `vendor` settled; when the count reaches zero every
    /// waiting callback runs.
    fn settle(vendor: Vendor, count: usize) {
        let finished = STATE.with(|state| {
            let mut state = state.borrow_mut();
            match state.inflight.get_mut(&vendor) {
                Some(left) => {
                    *left = left.saturating_sub(count);
                    *left == 0
                }
                // Bundle already finished (or was never started): nothing to do.
                None => false,
            }
        });
        if finished {
            run_pending(vendor);
        }
    }

    /// Mark `vendor` done and drain its callbacks — always outside the state
    /// borrow, since a callback may re-enter [`ensure`] for another bundle.
    fn run_pending(vendor: Vendor) {
        let callbacks = STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.done.insert(vendor);
            state.pending.remove(&vendor).unwrap_or_default()
        });
        for callback in callbacks {
            callback();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const ALL: [Vendor; 4] = [
        Vendor::Leaflet,
        Vendor::JsBarcode,
        Vendor::Quill,
        Vendor::Marked,
    ];

    /// The in-flight counter is per bundle, so two bundles sharing an asset
    /// URL would double-count (or never finish) depending on injection order.
    /// The table below must stay disjoint.
    #[test]
    fn disjoint_bundles_do_not_share_urls() {
        let mut seen = HashSet::new();
        for vendor in ALL {
            for url in vendor.styles().iter().chain(vendor.scripts()) {
                assert!(
                    seen.insert(*url),
                    "{url} appears in two bundles — per-bundle settle counting would hang"
                );
            }
        }
    }

    /// Every URL the loader injects must be served by the embedded table with
    /// the matching content type: the loader and the route handler live in
    /// different crates and this is the seam between them.
    #[test]
    fn every_bundle_asset_is_served_by_the_server_table() {
        for vendor in ALL {
            for url in vendor.styles() {
                let (content_type, _) = crate::vendor_assets::lookup(url)
                    .unwrap_or_else(|| panic!("style {url} not in vendor_assets"));
                assert!(
                    content_type.starts_with("text/css"),
                    "{url} should be CSS, got {content_type}"
                );
            }
            for url in vendor.scripts() {
                let (content_type, _) = crate::vendor_assets::lookup(url)
                    .unwrap_or_else(|| panic!("script {url} not in vendor_assets"));
                assert!(
                    content_type.starts_with("application/javascript"),
                    "{url} should be JS, got {content_type}"
                );
            }
        }
    }

    /// Widgets branch on [`Vendor::global`] before touching a library — an
    /// empty or misspelled name would make them fall back forever.
    #[test]
    fn every_bundle_exposes_a_named_global() {
        for vendor in ALL {
            assert!(!vendor.global().is_empty(), "{vendor:?} has no global name");
        }
        assert_eq!(Vendor::Leaflet.global(), "L");
        assert_eq!(Vendor::JsBarcode.global(), "JsBarcode");
    }
}
