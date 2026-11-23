//! `Text Editor` (Quill) and `Markdown Editor` (marked) fields.
//!
//! Both store plain strings — HTML for Quill, Markdown source for the other —
//! so the wire contract does not change; only the widget does. Both libraries
//! arrive lazily through [`crate::ext::vendor`]; until then SSR renders the
//! raw HTML div (Quill takes it over as initial content) and the plain
//! textarea (Markdown), and every failure path lands on a loud console error
//! plus the raw markup instead of a dead preview.

use leptos::prelude::*;

use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::textarea::Textarea;
use crate::ext::vendor::Vendor;
use crate::i18n::t;
// The escaping bridge only exists on wasm and in the unit tests; the native
// build has no `marked` renderer to install it into.
#[cfg(any(target_arch = "wasm32", test))]
use ferris_web_shared::html::escape_html;
// Sanitizing is platform-neutral: the SSR HTML and the hydrated editor must be
// painted from the same filtered value (CMP-10).
use ferris_web_shared::html::sanitize_html;

/// Toolbar + theme for the Quill instance. A fixed const, never user data,
/// which is what keeps the options one `JSON::parse` instead of a `Reflect`
/// tree — and what makes the inline toolbar (`style-src 'unsafe-inline'` is
/// already in the CSP) safe to ship. wasm-and-tests only: a native build has
/// no editor to configure.
#[cfg(any(target_arch = "wasm32", test))]
const QUILL_OPTIONS: &str = r#"{"theme":"snow","placeholder":"__FERRIS_PLACEHOLDER__","modules":{"toolbar":[["bold","italic","underline","link"],["blockquote","code-block"],[{"list":"ordered"},{"list":"bullet"}],["clean"]]}}"#;

/// Rich text (`Text Editor`): Quill over a contenteditable div.
///
/// The value is the editor's HTML, so it round-trips unchanged through the
/// API; Quill's own clipboard sanitizes what comes in from a paste.
///
/// The value is painted with `inner_html`, so it is filtered through
/// [`sanitize_html`] first: a stored document body is data, and data must not
/// be able to run script in the Desk (CMP-10). Quill then owns the subtree as
/// usual; the sanitized value is what both SSR and the first hydrate render.
#[component]
pub fn TextEditorField(id: String, value: RwSignal<String>, read_only: bool) -> impl IntoView {
    let editor_id = format!("{id}-quill");
    let i18n = crate::i18n::use_i18n();
    let placeholder = i18n.t("components.form.editor.placeholder");
    // Rendered once: Quill owns this subtree from `init` onwards, so an
    // attribute that kept re-rendering would fight its DOM. Parity holds
    // because server and client render the same *untracked* initial value.
    let initial = sanitize_html(&value.get_untracked());
    {
        let init_id = editor_id.clone();
        Effect::new(move |_| init_when_ready(&init_id, value, read_only, placeholder.clone()));
    }
    // External changes (document reload, fetch_from, another widget) flow
    // back into the editor; changes made here flow out through Quill's own
    // `text-change`, installed in `init`.
    let sync_id = editor_id.clone();
    Effect::new(move |_| apply_external(&sync_id, &value.get()));
    // The cleanup runs before the view consumes `editor_id`, so it gets its
    // own copy.
    let cleanup_id = editor_id.clone();
    on_cleanup(move || dispose_editor(&cleanup_id));

    view! {
        <div class="flex flex-col gap-2 ferris-quill" data-name="__TextEditorField">
            // The editor's own face: 16px on a phone, because a smaller
            // contenteditable makes iOS Safari zoom the page on focus.
            <div id=editor_id class="text-sm max-md:text-base" inner_html=initial></div>
        </div>
    }
}

/// Build the Quill instance the first time the bundle is on the page.
#[cfg(target_arch = "wasm32")]
fn init_when_ready(editor_id: &str, value: RwSignal<String>, read_only: bool, placeholder: String) {
    let editor_id = editor_id.to_string();
    crate::ext::vendor::ensure(Vendor::Quill, move || {
        wasm::init_editor(&editor_id, value, read_only, &placeholder);
    });
}

/// Native twin: SSR renders the raw HTML; effects never run there.
#[cfg(not(target_arch = "wasm32"))]
fn init_when_ready(
    _editor_id: &str,
    _value: RwSignal<String>,
    _read_only: bool,
    _placeholder: String,
) {
}

/// Paste an external value into the editor, if it is there and differs.
///
/// The value goes through [`sanitize_html`] on the way in: Quill's clipboard
/// conversion is not a security boundary, and an external load (document
/// reload, `fetch_from`) can carry a body this Desk never wrote (CMP-10).
#[cfg(target_arch = "wasm32")]
fn apply_external(editor_id: &str, html: &str) {
    wasm::apply_external(editor_id, &sanitize_html(html));
}

/// Native twin of `apply_external`.
#[cfg(not(target_arch = "wasm32"))]
fn apply_external(_editor_id: &str, _html: &str) {}

/// Drop the editor handle once its subtree leaves the document.
#[cfg(target_arch = "wasm32")]
fn dispose_editor(editor_id: &str) {
    wasm::dispose(editor_id);
}

/// Native twin of `dispose_editor`.
#[cfg(not(target_arch = "wasm32"))]
fn dispose_editor(_editor_id: &str) {}

/// Markdown (`Markdown Editor`): a textarea and, on toggle, a sanitized
/// preview rendered with marked.
#[component]
pub fn MarkdownField(
    id: String,
    value: RwSignal<String>,
    read_only: bool,
    placeholder: String,
    #[prop(default = 6u32)] rows: u32,
    #[prop(default = None)] max_length: Option<u32>,
) -> impl IntoView {
    let toggle_i18n = crate::i18n::use_i18n();
    let showing = RwSignal::new(false);
    let preview = RwSignal::new(String::new());
    let loading = RwSignal::new(false);
    // One effect covers both triggers: the toggle flips `showing` and a
    // document change rewrites `value`, and refresh() itself defers behind
    // the bundle the first time.
    Effect::new(move |_| {
        if showing.get() {
            refresh(&preview, &loading, &value.get());
        }
    });

    view! {
        <div class="flex flex-col gap-2" data-name="__MarkdownField">
            <Show
                when=move || showing.get()
                fallback=move || view! {
                    <Textarea
                        id=id.clone()
                        bind_value=value
                        readonly=read_only
                        placeholder=placeholder.clone()
                        rows=rows
                        class="font-mono text-sm max-md:text-base"
                        attr:maxlength=max_length
                    />
                }
            >
                <div class="min-h-24 rounded-md border bg-card p-3 text-sm leading-relaxed text-card-foreground">
                    <Show
                        when=move || loading.get()
                        fallback=move || view! { <div inner_html=move || preview.get()></div> }
                    >
                        <p class="text-sm text-muted-foreground">{t("components.form.field.preview_loading")}</p>
                    </Show>
                </div>
            </Show>
            <div class="flex justify-end">
                <Button
                    variant=ButtonVariant::Ghost
                    size=ButtonSize::Sm
                    on:click=move |_| showing.update(|s| *s = !*s)
                >
                    {move || {
                        if showing.get() {
                            toggle_i18n.t("components.form.field.edit")
                        } else {
                            toggle_i18n.t("components.form.field.preview")
                        }
                    }}
                </Button>
            </div>
        </div>
    }
}

/// Parse `raw` into the preview, or queue this refresh behind the bundle the
/// first time. A load that settles without a usable `parse` ends the
/// recursion with a loud console error and an empty preview — never an
/// endless spinner and never stale content.
fn refresh(preview: &RwSignal<String>, loading: &RwSignal<bool>, raw: &str) {
    if let Some(html) = parse_markdown(raw) {
        preview.set(html);
        loading.set(false);
    } else if crate::ext::vendor::is_ready(Vendor::Marked) {
        log_error("markdown preview: the marked bundle settled without a usable parse()");
        preview.set(String::new());
        loading.set(false);
    } else {
        loading.set(true);
        let next_preview = *preview;
        let next_loading = *loading;
        let raw = raw.to_string();
        crate::ext::vendor::ensure(Vendor::Marked, move || {
            refresh(&next_preview, &next_loading, &raw);
        });
    }
}

/// `marked.parse(raw)` as a string; `None` before the bundle ran or when the
/// global is unusable.
#[cfg(target_arch = "wasm32")]
fn parse_markdown(raw: &str) -> Option<String> {
    let marked = crate::ext::vendor::global(Vendor::Marked)?;
    install_html_escaping(&marked);
    let html =
        crate::ext::vendor::js::call(&marked, "parse", &[wasm_bindgen::JsValue::from_str(raw)])?;
    html.as_string()
}

/// Native twin: there is no `marked`, and no preview either (effects and
/// clicks do not happen during SSR).
#[cfg(not(target_arch = "wasm32"))]
fn parse_markdown(_raw: &str) -> Option<String> {
    None
}

/// `marked` renders raw HTML tokens verbatim; the Desk escapes them so a
/// hostile document body cannot smuggle markup into the preview (the CSP
/// would already block inline handlers, this removes the payload too).
///
/// Installed once per page — the renderer override belongs to the library,
/// not to the field — and the wasm closure is leaked on purpose: `marked`
/// keeps calling it for the life of the page, so it must never be dropped.
#[cfg(target_arch = "wasm32")]
fn install_html_escaping(marked: &wasm_bindgen::JsValue) {
    use std::sync::OnceLock;
    use wasm_bindgen::{JsValue, closure::Closure};

    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let escape = Closure::<dyn FnMut(JsValue) -> JsValue>::new(|token: JsValue| {
            let text = crate::ext::vendor::js::get(&token, "text")
                .and_then(|t| t.as_string())
                .unwrap_or_default();
            JsValue::from_str(&escape_html(&text))
        });
        let renderer = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&renderer, &JsValue::from_str("html"), escape.as_ref());
        // `marked` owns the renderer object and will call this for as long as
        // the page lives: forgetting it here is what keeps that call valid.
        escape.forget();
        let options = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&options, &JsValue::from_str("renderer"), &renderer);
        let _ = crate::ext::vendor::js::call(marked, "use", &[options.into()]);
    });
}

#[cfg(target_arch = "wasm32")]
fn log_error(message: &str) {
    crate::ext::vendor::js::error(message);
}

#[cfg(not(target_arch = "wasm32"))]
fn log_error(_message: &str) {}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use leptos::prelude::*;
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};

    use super::QUILL_OPTIONS;
    use crate::ext::vendor::js;
    use crate::ext::vendor::{self, Vendor};

    /// One live editor. The `text-change` closure is dropped only once the
    /// editor is out of the document and unreachable, so no event can fire
    /// into an invalidated wasm closure.
    struct EditorHandle {
        quill: JsValue,
        _on_change: Closure<dyn FnMut(JsValue)>,
    }

    thread_local! {
        static EDITORS: RefCell<HashMap<String, EditorHandle>> = RefCell::new(HashMap::new());
    }

    pub(super) fn init_editor(
        editor_id: &str,
        value: RwSignal<String>,
        read_only: bool,
        placeholder: &str,
    ) {
        if EDITORS.with(|editors| editors.borrow().contains_key(editor_id)) {
            return;
        }
        let Some(quill_ctor) = vendor::global(Vendor::Quill) else {
            js::error("text editor: the Quill global is missing after load");
            return;
        };
        let Some(container) = document().and_then(|doc| doc.get_element_by_id(editor_id)) else {
            return; // unmounted before the bundle landed
        };
        // The options are a const, so the parse cannot fail — if it ever does,
        // that is a build bug and it must be loud.
        // The placeholder is the only user/locale string in the options; the
        // rest is the fixed toolbar. `serde_json` quotes it, so a translation
        // with quotes cannot break the JSON.
        let placeholder = serde_json::to_string(placeholder).unwrap_or_else(|_| "\"\"".to_string());
        let options_json = QUILL_OPTIONS.replace("\"__FERRIS_PLACEHOLDER__\"", &placeholder);
        let Ok(options) = js_sys::JSON::parse(&options_json) else {
            js::error("text editor: the QUILL_OPTIONS template does not parse");
            return;
        };
        let Some(quill) = js::construct(&quill_ctor, &[container.into(), options]) else {
            js::error("text editor: `new Quill(...)` failed");
            return;
        };
        if read_only {
            let _ = js::call(&quill, "enable", &[JsValue::FALSE]);
        }
        let on_change = Closure::<dyn FnMut(JsValue)>::new({
            let quill = quill.clone();
            move |_: JsValue| {
                let Some(html) = editor_html(&quill) else {
                    return;
                };
                // The paste in `apply_external` fires this too; its result
                // already matches the signal, so the loop stops there.
                if value.get_untracked() != html {
                    value.set(html);
                }
            }
        });
        let _ = js::call(
            &quill,
            "on",
            &[JsValue::from_str("text-change"), on_change.as_ref().clone()],
        );
        EDITORS.with(|editors| {
            editors.borrow_mut().insert(
                editor_id.to_string(),
                EditorHandle {
                    quill,
                    _on_change: on_change,
                },
            );
        });
    }

    /// Push an external value into the editor; the handler above normalizes
    /// once more and settles.
    pub(super) fn apply_external(editor_id: &str, html: &str) {
        let quill =
            EDITORS.with(|editors| editors.borrow().get(editor_id).map(|h| h.quill.clone()));
        let Some(quill) = quill else { return };
        if editor_html(&quill).as_deref() == Some(html) {
            return;
        }
        let Some(clipboard) = js::get(&quill, "clipboard") else {
            return;
        };
        let Some(delta) = js::call(&clipboard, "convert", &[JsValue::from_str(html)]) else {
            return;
        };
        let _ = js::call(&quill, "setContents", &[delta]);
    }

    pub(super) fn dispose(editor_id: &str) {
        EDITORS.with(|editors| editors.borrow_mut().remove(editor_id));
    }

    fn editor_html(quill: &JsValue) -> Option<String> {
        let root = js::get(quill, "root")?;
        root.dyn_into::<web_sys::HtmlElement>()
            .ok()
            .map(|root| root.inner_html())
    }

    fn document() -> Option<web_sys::Document> {
        web_sys::window().and_then(|window| window.document())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_markup_in_html_tokens() {
        assert_eq!(
            escape_html(r#"<img src=x onerror="pwn()">"#),
            "&lt;img src=x onerror=&quot;pwn()&quot;&gt;"
        );
        assert_eq!(escape_html("plain & safe"), "plain &amp; safe");
        assert_eq!(escape_html("sin marcas"), "sin marcas");
    }

    /// The Quill options are a const parsed with `JSON::parse` on every
    /// init: if the toolbar spec ever drifts from JSON, every editor on the
    /// Desk silently refuses to open.
    #[test]
    fn quill_options_are_valid_json() {
        let options: serde_json::Value =
            serde_json::from_str(QUILL_OPTIONS).expect("QUILL_OPTIONS must stay valid JSON");
        assert_eq!(options["theme"], "snow");
        assert!(options["modules"]["toolbar"].is_array());
    }

    /// CMP-10: the value the editor paints is the sanitized one, so a stored
    /// body carrying a handler or a script cannot reach `inner_html`.
    #[test]
    fn the_editor_paints_only_sanitized_html() {
        assert_eq!(
            sanitize_html(r#"<p onclick="pwn()">ok</p><script>alert(1)</script>"#),
            "<p>ok</p>"
        );
        assert_eq!(
            sanitize_html("<p>hola <strong>mundo</strong></p>"),
            "<p>hola <strong>mundo</strong></p>"
        );
    }
}
