//! The two *other* ways out of the layout editor, in the browser only.
//!
//! The menu row and the top bar's own navigation go through
//! [`super::leave_dialog`], which owns the confirmation. Everything else — the
//! breadcrumb back to the `DocType`'s list, a sidebar link, the browser's reload
//! or close — never reaches the Desk's code, so it is caught here:
//!
//! * a capture-phase `click` listener on the document stops an in-app link
//!   while the editor holds a layout session, remembers where the user meant to
//!   go and opens the same dialog;
//! * a `beforeunload` listener makes the browser ask its own question when the
//!   page is reloaded or closed (a page unload has no dialog of ours to show).
//!
//! Both listeners are installed once per bar and decide per event from the live
//! service, so none of them has to be re-attached when the stacks move. A
//! plain, session-less `Ctrl+Z`-free read of the document editor never trips
//! them: the predicate is "layout route **and** something to undo".
//!
//! Known limit: the browser's own back/forward buttons navigate through the
//! History API, which cannot be cancelled. They are covered only by the
//! `beforeunload` prompt when they leave the app.

// The whole module is browser-only; on the SSR build the imports and the
// functions would be unused, and `unused_imports` is a warning here.
#[cfg(target_arch = "wasm32")]
use crate::views::services::FormService;
#[cfg(target_arch = "wasm32")]
use leptos::prelude::*;

/// Install the link and unload guards for the lifetime of the form bar.
#[cfg(target_arch = "wasm32")]
pub(super) fn install_leave_guard(
    service: StoredValue<FormService>,
    leave_open: RwSignal<bool>,
    leave_target: RwSignal<String>,
) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    // The live predicate. Tracked nowhere: every listener asks again when it
    // fires, so a stack that moved after install is still honoured.
    let guarded = move || {
        service
            .try_with_value(|svc| svc.layout_mode.get_untracked() && svc.has_layout_history())
            .unwrap_or(false)
    };

    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };

    // ---- In-app links (capture, so the router never sees the click) ----
    let click =
        Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |event: web_sys::MouseEvent| {
            if !guarded() || event.default_prevented() || event.button() != 0 {
                return;
            }
            // A modified click opens a tab/window: the current page stays, so
            // there is nothing to confirm. Shift+click is a new window too.
            if event.ctrl_key() || event.meta_key() || event.shift_key() || event.alt_key() {
                return;
            }
            let Some(target) = event.target() else {
                return;
            };
            let Some(element) = target.dyn_ref::<web_sys::Element>() else {
                return;
            };
            let Ok(Some(anchor)) = element.closest("a[href]") else {
                return;
            };
            let Some(anchor) = anchor.dyn_ref::<web_sys::HtmlAnchorElement>() else {
                return;
            };
            let target_attr = anchor.target();
            if !target_attr.is_empty() && target_attr != "_self" {
                return;
            }
            let href = anchor.get_attribute("href").unwrap_or_default();
            // Only in-app paths: an absolute URL leaves the Desk entirely and
            // is the browser's business, and `#`/`mailto:` do not navigate.
            if !href.starts_with('/') || href.starts_with("//") || href.starts_with("/#") {
                return;
            }
            let current = web_sys::window()
                .and_then(|window| window.location().pathname().ok())
                .unwrap_or_default();
            if same_path(&href, &current) {
                return;
            }
            event.prevent_default();
            event.stop_propagation();
            leave_target.set(href);
            leave_open.set(true);
        });
    let _ = document.add_event_listener_with_callback_and_bool(
        "click",
        click.as_ref().as_ref().unchecked_ref(),
        true,
    );

    // ---- Reload / close / external back ----
    // `prevent_default` plus an empty `return_value` is the portable spelling of
    // "ask the user"; the browser owns the wording and there is no way for the
    // page to style it.
    let unload = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        if !guarded() {
            return;
        }
        event.prevent_default();
        if let Some(unload) = event.dyn_ref::<web_sys::BeforeUnloadEvent>() {
            unload.set_return_value("");
        }
    });
    let _ =
        document.add_event_listener_with_callback("beforeunload", unload.as_ref().unchecked_ref());

    // `LocalStorage` keeps the non-`Send` closures in the reactive arena so the
    // cleanup can take them back; forgetting them would leak a handler that
    // outlives the form. The tuple type is inferred from the values, which is
    // also why no alias is needed here.
    let listeners = StoredValue::new_local(Some((document, click, unload)));

    on_cleanup(move || {
        let Some((document, click, unload)) = listeners.try_update_value(Option::take).flatten()
        else {
            return;
        };
        let _ = document.remove_event_listener_with_callback_and_bool(
            "click",
            click.as_ref().as_ref().unchecked_ref(),
            true,
        );
        let _ = document
            .remove_event_listener_with_callback("beforeunload", unload.as_ref().unchecked_ref());
    });
}

/// Whether `href` addresses the same page as `current`, ignoring a query
/// string, a fragment and a trailing slash.
///
/// A link to the page the user is already on is not a way out of the editor, so
/// the confirmation must not appear for it.
#[cfg(target_arch = "wasm32")]
fn same_path(href: &str, current: &str) -> bool {
    let path = |raw: &str| {
        raw.split(['?', '#'])
            .next()
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string()
    };
    path(href) == path(current)
}
