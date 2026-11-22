//! Server-side shell that bootstraps the desk for SSR.

use leptos::config::LeptosOptions;
use leptos::prelude::*;
use leptos_meta::{MetaTags, provide_meta_context};
use leptos_router::components::Router;

use crate::app::DeskApp;
use crate::web_user::WebUser;

/// The `id`/`name` of the `<meta>` element the shell stamps with the identity it
/// rendered. [`crate::hydrate`] reads it back so the first client render matches
/// the server's (no flash of the login page for an already-signed-in visitor).
pub const BOOTSTRAP_USER_META: &str = "ferris-user";

/// Keep the bootstrap `<meta>` in step with the identity the Desk is showing.
///
/// The meta is the server's snapshot, read once by [`crate::hydrate`] to make
/// the first client render match SSR. A SPA sign-in (or sign-out) changes the
/// identity without a new document, so without this the tag would keep saying
/// `Guest` while the sidebar says `Administrator` — and a plugin reading the
/// tag at runtime would see the wrong user. It is not a security boundary: the
/// cookie and the server-side session still decide every request.
#[cfg(target_arch = "wasm32")]
pub fn stamp_bootstrap_user(user: &WebUser) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let selector = format!("meta[name=\"{BOOTSTRAP_USER_META}\"]");
    let Ok(Some(meta)) = document.query_selector(&selector) else {
        return;
    };
    let Ok(encoded) = serde_json::to_string(user) else {
        return;
    };
    let _ = meta.set_attribute("content", &encoded);
}

/// Renders the desk as a full HTML document: provides the identity signal, a
/// single `MetaContext`, the `<Router>` required by `use_location` and the
/// hydration scripts that bring the page to life on the client (Fase 7). The
/// explicit `<head>`/`</head>` is where `leptos_meta` injects `<Title>`/`<Meta>`
/// during SSR.
///
/// The identity comes from the `WebUser` context the server's request guard
/// provides (see `ferris_server::router::provide_desk_context`); when there is
/// none — the startup `generate_route_list` render, a test — it falls back to
/// the guest, which is what the login page renders as.
///
/// `MetaTags` must come first inside `<head>`: its server-renderer emits the
/// `<!--HEAD-->` marker that `leptos_meta` later fills with the registered tags
/// and that the client-side `MetaContext` uses as its hydration cursor. If it
/// is placed last (after `<HydrationScripts/>`) the marker is not emitted and
/// hydration panics looking for it.
#[component]
pub fn DeskShell() -> impl IntoView {
    provide_meta_context();

    let web_user = leptos::context::use_context::<WebUser>().unwrap_or_else(WebUser::guest);
    provide_context(RwSignal::new(web_user.clone()));
    // Serialized for the client. A `<meta>` (rather than a `<script>`) keeps it
    // outside the CSP `script-src` gate and out of `hydrate_body`'s way.
    let bootstrap = serde_json::to_string(&web_user).unwrap_or_else(|_| "null".to_string());

    // Injected by `leptos_axum` while SSR-rendering a request. Leaked during
    // the startup `generate_route_list` render call, where we fall back to the
    // same values the server config uses.
    let options = leptos::context::use_context::<LeptosOptions>()
        .unwrap_or_else(|| LeptosOptions::builder().output_name("ferris-web").build());

    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <MetaTags />
                <meta name=BOOTSTRAP_USER_META content=bootstrap />
                <link rel="stylesheet" href="/static/desk.css" />
                <HydrationScripts options=options.clone() />
                <AutoReload options=options />
            </head>
            <body>
                <Router>
                    <DeskApp />
                </Router>
            </body>
        </html>
    }
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::*;
    use leptos_axum::generate_route_list;

    /// Every desk route must appear in the SSR route list, otherwise the server
    /// answers direct navigation to deep links (e.g. `/desk/test/Task`) with a
    /// 404 instead of rendering the shell. The router only finds `<Route/>`
    /// children that are statically present in the tree — they must not live
    /// behind reactive conditionals.
    #[test]
    fn route_list_covers_desk_deep_links() {
        let routes = generate_route_list(|| view! { <DeskShell /> });
        let listing: Vec<String> = routes.iter().map(|r| r.path().to_string()).collect();
        for path in [
            "/desk/{module}/{doctype}",
            "/desk/{module}/{doctype}/{name}",
            "/report/{name}",
            "/kanban/{doctype}",
            "/settings/{*rest}",
            // Fase 2: the Desk's client-side catch-all for plugin pages. It is
            // filtered out of the axum table in `ferris-server` (only the
            // paths plugins declare are registered concretely), but it must
            // exist in this list or the filter has nothing to remove.
            "/{*rest}",
        ] {
            assert!(
                listing.iter().any(|r| r == path),
                "deep-link route {path:?} missing from SSR route list: {listing:?}"
            );
        }
    }
}
