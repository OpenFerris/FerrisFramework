//! App-declared custom doctype views: the [`SLOT_FORM_VIEW`] mount point.
//!
//! [`SLOT_FORM_VIEW`]: crate::ext::SLOT_FORM_VIEW
//!
//! An app opts a doctype out of the built-in metadata form by declaring a
//! `ui_extensions` entry whose `slot` is `"form.view"` and whose `doctype`
//! names it (`{ "slot": "form.view", "doctype": "Equipo", "component":
//! "equipo-editor" }`). The Desk then mounts `<ferris-equipo-editor>` as the
//! whole form body at `/desk/:module/:doctype/:name` (and for a Single at
//! `/desk/:module/:doctype`) instead of rendering the generic editor.
//!
//! The custom view is a first-class companion of [`crate::ext::UiSlot`]: it
//! goes through the same feed (`GET /api/ui-extensions`), the same asset loader
//! (the app's scripts and CSS are injected once and its stylesheet is scoped to
//! the anchor's shadow root), the same shadow-DOM isolation and the same
//! owner-sorted precedence. What it adds is the **document**: the metadata form
//! exists to edit one document, so a replacement has to receive it. The host
//! resolves the route's document (a Single through its single endpoint, `new`
//! as `null`) and hands it to the mounted element as `data-doc`, alongside the
//! declaration's `props` projected from that document (`data-props`, `"$field"`
//! values resolved against it). A `ferris-document` CustomEvent with the same
//! payload is dispatched once the element and the document are both ready, so a
//! component that prefers events over attribute reads can use one channel.
//!
//! SSR/hydration contract, identical to every other feed-driven surface: the
//! resolver reads the feed, which is empty on the server and on the client's
//! first pass, so both render the built-in form and only swap once the feed
//! lands. This component is therefore only ever mounted client-side.

use leptos::prelude::*;

#[cfg(target_arch = "wasm32")]
use crate::ext::AppUi;
use crate::ext::UiExtension;

/// The whole-view anchor for a doctype with an app-declared `form.view`.
///
/// `spec` is a signal (not a value) because the router reuses the route view
/// across param changes and the feed can hot-reload: reading it per run is what
/// lets the same component follow the extension and the document it points at.
#[component]
pub fn DoctypeViewHost(
    /// The declaring app plus its `form.view` declaration, or `None` when the
    /// feed no longer carries one (the parent then renders the built-in form).
    spec: Signal<Option<(String, UiExtension)>>,
    /// Current doctype (`/desk/:module/:doctype/:name`).
    doctype: Signal<String>,
    /// Current workspace/module segment of the route.
    module: Signal<String>,
    /// Document name; the doctype itself for a Single, `new` when creating.
    docname: Signal<String>,
) -> impl IntoView {
    let ctx = use_context::<crate::ext::UiCtx>();
    let anchor: NodeRef<leptos::html::Div> = NodeRef::new();

    // Guards interleaved navigations: the mount is async (the element may still
    // be loading and the document is fetched), so a second run must be able to
    // invalidate the first's pending write. `mine` is claimed per run and
    // checked after every await.
    //
    // A plain `Rc<Cell>` rather than a reactive signal on purpose: the pending
    // async outlives a run whose component may have been disposed (navigate to
    // another doctype, feed reload drops the view), and reading a disposed
    // signal panics. This token carries no reactivity, only the generation.
    let generation = std::rc::Rc::new(std::cell::Cell::new(0u32));

    Effect::new(move |_| {
        let Some((owner, view)) = spec.get() else {
            return;
        };
        let dt = doctype.get();
        let name = docname.get();
        let module = module.get();
        let mine = generation.get().wrapping_add(1);
        generation.set(mine);

        #[cfg(target_arch = "wasm32")]
        {
            let apps = ctx.map(|ctx| ctx.apps.get()).unwrap_or_default();
            crate::ext::loader::ensure_loaded(&apps);
            mount_doctype_view(
                anchor.get(),
                DocViewMount {
                    owner: &owner,
                    view: &view,
                    doctype: &dt,
                    name: &name,
                    module: &module,
                    apps: &apps,
                    generation: std::rc::Rc::clone(&generation),
                    mine,
                },
            );
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (anchor, owner, view, dt, name, module, mine, ctx);
    });

    view! {
        <div
            node_ref=anchor
            class="ferris-doctype-view"
            data-slot="form.view"
            data-doctype=move || doctype.get()
        ></div>
    }
}

/// Everything one mount needs, borrowed from the effect that scheduled it.
/// Bundled so the mount entry point takes one argument instead of a long list.
#[cfg(target_arch = "wasm32")]
struct DocViewMount<'a> {
    owner: &'a str,
    view: &'a UiExtension,
    doctype: &'a str,
    name: &'a str,
    module: &'a str,
    apps: &'a [AppUi],
    generation: std::rc::Rc<std::cell::Cell<u32>>,
    mine: u32,
}

/// Mount the app's custom element into the anchor's shadow root and hand it the
/// selected document.
///
/// Three asynchronous steps, each guarded by the run's generation so a faster
/// navigation is never overwritten by a slower earlier one:
///
/// 1. wait for the app's script to define the element (a placeholder shows
///    meanwhile — the same "Cargando…" the widget and website hosts use);
/// 2. read the document through [`crate::api::DocStore`] (a Single through its
///    single endpoint, `new` as `null`);
/// 3. create the element with the route, declaration and document attributes
///    (`data-doc`, `data-docstatus`, `data-props`, or `data-error`) and append
///    it, then dispatch `ferris-document`.
///
/// The document is resolved *before* the element exists so that its
/// `connectedCallback` already sees the data; the event fires *after* it is
/// connected, so a listener it registered there receives it.
#[cfg(target_arch = "wasm32")]
fn mount_doctype_view(anchor: Option<web_sys::HtmlDivElement>, mount: DocViewMount<'_>) {
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

    // Replacing the previous document's mount: a fresh element per navigation.
    while let Some(child) = container.first_child() {
        let _ = container.remove_child(&child);
    }

    // The declaring app's stylesheet, scoped to this view's shadow tree.
    let urls = crate::ext::loader::stylesheet_urls_for_owners(mount.apps, &[mount.owner]);
    crate::ext::loader::adopt(&shadow, &urls);

    let tag = crate::ext::component_tag(&mount.view.component);
    let registry = window.custom_elements();
    if registry.get(&tag).is_undefined() {
        container
            .set_inner_html(r#"<div class="p-6 text-sm text-muted-foreground">Cargando…</div>"#);
    }

    // Owned for the future, which outlives the borrowed mount input.
    let owner = mount.owner.to_string();
    let component = mount.view.component.clone();
    let label = mount.view.label.clone();
    let props = mount.view.props.clone();
    let doctype = mount.doctype.to_string();
    let name = mount.name.to_string();
    let module = mount.module.to_string();
    let generation = mount.generation;
    let mine = mount.mine;

    crate::api::drive(async move {
        match registry.when_defined(&tag) {
            Ok(defined) => {
                let _ = wasm_bindgen_futures::JsFuture::from(defined).await;
            }
            Err(_) => return,
        }
        if generation.get() != mine {
            return;
        }

        // Resolve the document **before** the element exists, so its
        // `connectedCallback` already reads `data-doc` — no create-then-patch
        // race, and a plugin can render from the data on first paint.
        let loaded = load_document(&doctype, &name).await;
        if generation.get() != mine {
            return;
        }

        // The container may have been rebuilt by a manifest refresh while the
        // script and document loaded; look it up again rather than trust the
        // old handle.
        let Some(shadow) = anchor.shadow_root() else {
            return;
        };
        let Some(container) = crate::ext::loader::mount_container(&shadow) else {
            return;
        };
        while let Some(child) = container.first_child() {
            let _ = container.remove_child(&child);
        }

        let Ok(el) = document.create_element(&tag) else {
            return;
        };
        let _ = el.set_attribute("data-slot", crate::ext::SLOT_FORM_VIEW);
        let _ = el.set_attribute("data-doctype", &doctype);
        let _ = el.set_attribute("data-name", &name);
        let _ = el.set_attribute("data-module", &module);
        let _ = el.set_attribute("data-owner", &owner);
        let _ = el.set_attribute("data-component", &component);
        if let Some(label) = &label {
            let _ = el.set_attribute("data-label", label);
        }

        let document_json = match loaded {
            Ok(doc) => {
                let json = serde_json::to_string(&doc).unwrap_or_else(|_| "null".to_string());
                let _ = el.set_attribute("data-doc", &json);
                let _ = el.set_attribute("data-docstatus", &docstatus_of(&doc));
                let resolved = crate::ext::resolve_props(&props, Some(&doc));
                if !resolved.is_empty() {
                    let json =
                        serde_json::to_string(&resolved).unwrap_or_else(|_| "[]".to_string());
                    let _ = el.set_attribute("data-props", &json);
                }
                Some(json)
            }
            Err(error) => {
                let _ = el.set_attribute("data-error", &error);
                None
            }
        };

        let _ = container.append_child(el.as_ref());
        // After `append_child`, `connectedCallback` has run and any listener it
        // registered is in place, so the event reaches the element.
        if let Some(json) = &document_json {
            dispatch_document(&el, &doctype, &name, &module, json);
        }
    });
}

/// Read the document the route names, through the platform [`DocStore`] (the
/// in-process engine on SSR, the REST client in the browser).
///
/// - `new` (and any other "no document yet" route) resolves to `null`, so a
///   custom view can render its own create experience;
/// - a Single is read through its single endpoint (`DocStore::get_single`);
/// - everything else through the ordinary document endpoint.
///
/// A read failure is returned, never swallowed: the caller surfaces it as
/// `data-error` instead of handing the element an empty document it would
/// mistake for a real one.
///
/// [`DocStore`]: crate::api::DocStore
#[cfg(target_arch = "wasm32")]
async fn load_document(doctype: &str, name: &str) -> Result<serde_json::Value, String> {
    let store = crate::api::DocStore::runtime()?;
    if name.is_empty() || name == "new" {
        return Ok(serde_json::Value::Null);
    }
    let meta = store
        .meta(doctype)
        .await?
        .ok_or_else(|| format!("Doctype {doctype} no registrado"))?;
    if meta.is_single {
        store.get_single(doctype).await
    } else {
        store.get(doctype, name).await
    }
}

/// The document's `docstatus` as the string the mounted element reads, `"0"`
/// when the document does not carry one (a `null`/create payload).
#[cfg(target_arch = "wasm32")]
fn docstatus_of(doc: &serde_json::Value) -> String {
    doc.get("docstatus")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
        .to_string()
}

/// Dispatch the `ferris-document` event with `{ doctype, name, module,
/// document }` in `detail`, once the element has both the document and a
/// listener. Best-effort: a browser that refuses `CustomEvent` leaves the
/// `data-doc` attribute as the channel, which is what the contract guarantees.
#[cfg(target_arch = "wasm32")]
fn dispatch_document(el: &web_sys::Element, doctype: &str, name: &str, module: &str, doc: &str) {
    use wasm_bindgen::JsValue;

    let detail = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &detail,
        &JsValue::from_str("doctype"),
        &JsValue::from_str(doctype),
    );
    let _ = js_sys::Reflect::set(
        &detail,
        &JsValue::from_str("name"),
        &JsValue::from_str(name),
    );
    let _ = js_sys::Reflect::set(
        &detail,
        &JsValue::from_str("module"),
        &JsValue::from_str(module),
    );
    // Parse the already-serialized document back into a JS value so the event
    // detail carries an object, not a JSON string.
    let document = js_sys::JSON::parse(doc).unwrap_or(JsValue::NULL);
    let _ = js_sys::Reflect::set(&detail, &JsValue::from_str("document"), &document);

    let init = web_sys::CustomEventInit::new();
    init.set_detail(&detail);
    if let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict("ferris-document", &init) {
        let _ = el.dispatch_event(&event);
    }
}
