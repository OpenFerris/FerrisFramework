//! The form's keyboard shortcuts, in the browser only.
//!
//! The listener lives in its own module because `form_top_bar.rs` is at the
//! repository's file-size budget and because it is the one part of the bar that
//! talks to the DOM directly. `form_view.rs` arranges the page, so the bar — the
//! thing that owns the undo/redo buttons — is where the keys that drive them
//! belong.

// The whole module is browser-only; on the SSR build the imports would be
// unused, and `unused_imports` is a warning in this workspace.
#[cfg(target_arch = "wasm32")]
use crate::views::services::FormService;
#[cfg(target_arch = "wasm32")]
use leptos::prelude::*;

/// Register `Ctrl+Z` / `Ctrl+Shift+Z` (`Cmd` on macOS) for the layout operation
/// stack.
///
/// Skipped while the event comes from an editable element: the browser's own
/// text undo is what the user expects inside a field, and hijacking it would
/// make the editor feel broken. The listener is removed on teardown, so a form
/// that unmounts does not leave a handler bound to a disposed service.
#[cfg(target_arch = "wasm32")]
pub(super) fn install_layout_shortcuts(service: StoredValue<FormService>) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    let handler =
        Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |event: web_sys::KeyboardEvent| {
            if !(event.ctrl_key() || event.meta_key()) || !event.key().eq_ignore_ascii_case("z") {
                return;
            }
            if event.target().is_some_and(|target| is_editable(&target)) {
                return;
            }
            let redo = event.shift_key();
            // `try_with_value`: the form may have unmounted between the key
            // press and this handler, and a disposed service is a no-op, never
            // a panic.
            //
            // The stacks exist only in the layout session, so outside `/layout`
            // the shortcut must not swallow the browser's own undo: the handler
            // reports whether it acted and only then cancels the event.
            let handled = service
                .try_with_value(|svc| {
                    if !svc.layout_mode.get_untracked() {
                        return false;
                    }
                    if redo {
                        svc.redo_layout();
                    } else {
                        svc.undo_layout();
                    }
                    true
                })
                .unwrap_or(false);
            if handled {
                event.prevent_default();
            }
        });

    // The DOM listener and its closure are not `Send`, while `on_cleanup`'s
    // callback is. `LocalStorage` keeps the pair in the reactive arena and
    // lets the copyable handle cross into the cleanup closure, instead of
    // leaking the handler with `forget`.
    type Listener = (
        web_sys::Document,
        Closure<dyn FnMut(web_sys::KeyboardEvent)>,
    );
    let listener = StoredValue::new_local(None::<Listener>);

    let document = web_sys::window().and_then(|window| window.document());
    if let Some(document) = document {
        let _ = document
            .add_event_listener_with_callback("keydown", handler.as_ref().as_ref().unchecked_ref());
        listener.set_value(Some((document, handler)));
    }
    on_cleanup(move || {
        if let Some((document, handler)) = listener.try_update_value(Option::take).flatten() {
            let _ = document.remove_event_listener_with_callback(
                "keydown",
                handler.as_ref().as_ref().unchecked_ref(),
            );
        }
    });
}

/// Whether `target` is a text surface the browser already gives `Ctrl+Z` to.
#[cfg(target_arch = "wasm32")]
fn is_editable(target: &web_sys::EventTarget) -> bool {
    use wasm_bindgen::JsCast;

    let Some(element) = target.dyn_ref::<web_sys::HtmlElement>() else {
        return false;
    };
    if element.is_content_editable() {
        return true;
    }
    matches!(
        element.tag_name().to_ascii_lowercase().as_str(),
        "input" | "textarea" | "select"
    )
}
