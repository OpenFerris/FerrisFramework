//! Client-side hydration bootstrap (wasm32-unknown-unknown, feature `hydrate`).
//!
//! Must be exported from the *lib* crate: `ferris dev --hot-reload frontend`
//! builds the front with `--lib`, so the wasm-bindgen glue
//! (`/pkg/ferris-web.js`) sees the exports declared here. The
//! `ferris-web-hydrate` binary exists only for standalone builds and delegates
//! to this function; those builds also carry its `main` as the wasm start, so
//! [`hydrate`] is idempotent (see [`HYDRATE_ONCE`]).

use std::sync::Once;

use leptos::prelude::*;
use leptos_router::components::Router;
use wasm_bindgen::prelude::*;

use crate::app::DeskApp;
use crate::components::hooks::use_scroll_lock;
use crate::shell::BOOTSTRAP_USER_META;
use crate::web_user::WebUser;

/// Guards against hydrating the same document twice.
///
/// The hydrate bundle can contain the `ferris-web-hydrate` binary's `main` as
/// the wasm start function (that is what a `--bin ferris-web-hydrate` build
/// produces). `wasm-bindgen` runs it while instantiating the module, and the
/// inline bootstrap then calls the exported `hydrate()` as well. Both calls
/// used to mount a full `<Router><DeskApp/></Router>` tree over the same DOM:
/// every listener was registered twice, a single sign-in click sent two
/// requests and created two sessions, and every boot fetch ran twice.
///
/// The guard makes the entry idempotent whichever build produced the bundle.
/// New builds should use `--lib` (no `main`/start), but an older or embedded
/// bundle must keep working rather than silently double-mounting.
static HYDRATE_ONCE: Once = Once::new();

/// Re-mounts the SSR-rendered desk on the browser side: every rust-ui component
/// is hydrated in place, with no flash of unstyled, non-interactive HTML.
///
/// Idempotent: the wasm start function and the inline bootstrap may both call
/// it (see [`HYDRATE_ONCE`]); only the first call mounts the app.
#[wasm_bindgen]
pub fn hydrate() {
    HYDRATE_ONCE.call_once(|| {
        console_error_panic_hook::set_once();
        // Register `window.ScrollLock` before any component script runs:
        // dropdowns, dialogs and sheets call it from their inline <script> tags.
        use_scroll_lock::init();
        let user = bootstrap_user();
        // The shell serialized the locale it rendered with; reading it back
        // here keeps the first client render in the same language as SSR
        // instead of falling back to the empty English catalogue.
        let i18n = crate::i18n::from_meta().unwrap_or_default();
        leptos::mount::hydrate_body(move || {
            // Seed with the identity the server rendered (stamped on the
            // bootstrap `<meta>`), so the first client render matches SSR.
            // `DeskApp` then revalidates with `/api/auth/session` and redirects
            // to `/login` only if the session is really gone.
            provide_context(RwSignal::new(user));
            provide_context(i18n);
            view! {
                <Router>
                    <DeskApp />
                </Router>
            }
        });
    });
}

/// The identity `DeskShell` rendered, read back from the bootstrap `<meta>`.
/// Falls back to the guest when the tag is missing or malformed (a shell
/// rendered outside a request, or a hand-written page).
fn bootstrap_user() -> WebUser {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return WebUser::guest();
    };
    let selector = format!("meta[name=\"{BOOTSTRAP_USER_META}\"]");
    let content = document
        .query_selector(&selector)
        .ok()
        .flatten()
        .and_then(|element| element.get_attribute("content"));
    content
        .and_then(|json| serde_json::from_str::<WebUser>(&json).ok())
        .unwrap_or_else(WebUser::guest)
}
