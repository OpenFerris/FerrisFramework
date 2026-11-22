//! Fase 2: plugin dashboards and website pages as first-class host routes.
//!
//! Plugins declare `dashboards` (each with a `route`) and `website.routes` in
//! `describe()`; `ferris-server` registers those declared paths for SSR, and
//! the Desk matches them with one trailing catch-all `<Route>` — the only
//! static tree position that can cover arbitrary plugin paths. This module is
//! that catch-all view: it resolves the current path against the registered
//! UI sets (`GET /api/ui-extensions`) and hands the dashboard's grid of
//! widget instances to [`WidgetHost`] (Fase 1), which resolves, frames and
//! mounts each one — or renders the website page the plugin declared.
//!
//! Hydration contract, same as [`crate::ext::UiSlot`]: SSR has no feed, so
//! both sides render the empty shell first and the client fills it in once
//! the feed lands. Widget bodies and page components are mounted from effects,
//! so they never appear in the server markup to disagree with the browser.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_location;

use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::icon::Icon;
use crate::components::ui::skeleton::Skeleton;
use crate::views::views_components::{EmptyState, Feedback, FeedbackKind};

use crate::ext::{AppUi, Dashboard, UiCtx, WebsiteRoute};
use crate::views::DashboardGrid;

/// The catch-all route view: current path → declared dashboard, declared
/// website route, or a small "not found" panel (only reachable client-side,
/// since the server registers just the declared paths).
#[component]
pub fn PluginPage() -> impl IntoView {
    let ctx = expect_context::<UiCtx>();
    let location = use_location();

    view! {
        <div class="ferris-plugin-page h-full min-h-0">
            {move || render_page(&ctx, &location.pathname.get())}
        </div>
    }
}

/// Which page the current path resolves to, decided before any view is
/// built so the decision itself is unit-testable.
enum Page<'a> {
    /// The feed has not arrived yet: what SSR and the client's first pass
    /// render, identically on both sides.
    Placeholder,
    Dashboard(&'a str, &'a Dashboard),
    Website(&'a str, &'a WebsiteRoute),
    /// Only reachable client-side: the server registers just the declared
    /// paths, so an unknown one never gets this far on a direct load.
    NotFound,
}

fn resolve_page<'a>(apps: &'a [AppUi], path: &str) -> Page<'a> {
    if apps.is_empty() {
        return Page::Placeholder;
    }
    if let Some((owner, dashboard)) = find_dashboard(apps, path) {
        return Page::Dashboard(owner, dashboard);
    }
    if let Some((owner, route)) = find_website_route(apps, path) {
        return Page::Website(owner, route);
    }
    Page::NotFound
}

/// Build the view for [`resolve_page`]'s decision.
fn render_page(ctx: &UiCtx, path: &str) -> AnyView {
    let apps = ctx.apps.get();
    if apps.is_empty() {
        // The feed has not landed, or it failed: a skeleton means "loading", an
        // error means "it did not load". Drawing the first for both is what
        // made a broken `/api/ui-extensions` look like a page that never
        // finished.
        return ctx
            .error
            .get()
            .map_or_else(placeholder, |message| feed_error(*ctx, message));
    }
    match resolve_page(&apps, path) {
        // Only possible with an empty feed, handled above; kept so the match
        // stays exhaustive over the resolution enum.
        Page::Placeholder => placeholder(),
        Page::Dashboard(owner, dashboard) => {
            // Same gutter every other page has, and a flex column so the
            // board's `h-full` still resolves against a definite height.
            view! {
                <div class="flex h-full min-h-0 flex-col p-4">
                    {dashboard_view(owner, dashboard)}
                </div>
            }
            .into_any()
        }
        Page::Website(owner, route) => website_view(owner, route).into_any(),
        Page::NotFound => view! {
            <div class="p-4">
                <EmptyState
                    icon="FileQuestion"
                    title="Página no encontrada"
                    description=format!("Ninguna app ha declarado una página en {path}.")
                >
                    <A
                        href=crate::app::DESK_PATH
                        attr:class="inline-flex h-9 max-md:h-10 items-center gap-2 rounded-md bg-primary px-4 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                    >
                        <Icon name="House" class="size-4" />
                        "Ir al escritorio"
                    </A>
                </EmptyState>
            </div>
        }
        .into_any(),
    }
}

/// What a page shows when the app feed failed: the reason and a retry, instead
/// of a loading skeleton that never ends.
pub(crate) fn feed_error(ctx: UiCtx, message: String) -> AnyView {
    view! {
        <div class="flex flex-col gap-3 p-4">
            <Feedback
                kind=FeedbackKind::Error
                message=Signal::derive(move || Some(message.clone()))
            />
            <p class="text-sm text-muted-foreground">
                "Esta página se apoya en el catálogo de extensiones de las apps."
            </p>
            <Button
                variant=ButtonVariant::Outline
                class="w-fit"
                on:click=move |_| ctx.refresh()
            >
                <Icon name="RefreshCw" class="size-4" />
                "Reintentar"
            </Button>
        </div>
    }
    .into_any()
}

/// The empty shell both SSR and the client's first pass render: the feed has
/// not arrived on either side yet, so there is nothing to show — and nothing
/// for hydration to disagree about. Shared with [`crate::views::HomeView`],
/// whose `/` is empty for exactly as long.
pub(crate) fn placeholder() -> AnyView {
    // The feed has not arrived yet. A skeleton keeps the layout stable between
    // SSR and hydration instead of a bare dashed box that looks like an error.
    view! {
        <div class="flex flex-col gap-4" aria-busy="true" aria-live="polite">
            <Skeleton class="h-8 w-full max-w-48" />
            <Skeleton class="h-4 w-full max-w-80" />
            <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
                {(0..3)
                    .map(|_| view! { <Skeleton class="h-28 w-full rounded-xl" /> })
                    .collect_view()}
            </div>
        </div>
    }
    .into_any()
}

/// The app and dashboard whose declared `route` is exactly `path`. A
/// dashboard with an empty `route` has no URL to visit, so it never matches.
fn find_dashboard<'a>(apps: &'a [AppUi], path: &str) -> Option<(&'a str, &'a Dashboard)> {
    apps.iter().find_map(|app| {
        app.dashboards
            .iter()
            .find(|dashboard| !dashboard.route.is_empty() && dashboard.route == path)
            .map(|dashboard| (app.owner.as_str(), dashboard))
    })
}

/// The app and website route matching `path` (`:params` match any segment).
fn find_website_route<'a>(apps: &'a [AppUi], path: &str) -> Option<(&'a str, &'a WebsiteRoute)> {
    apps.iter().find_map(|app| {
        app.website.as_ref().and_then(|website| {
            website
                .routes
                .iter()
                .find(|route| route_matches(&route.path, path))
                .map(|route| (app.owner.as_str(), route))
        })
    })
}

/// Whether a declared website page at `path` asked to draw its own surface.
///
/// A website route's `kind` is `"page"` by default; `"bare"` (any case) means
/// the page owns the whole viewport, so the Desk leaves out the sidebar and the
/// top bar. Matching uses the same shape rules as [`find_website_route`], so a
/// bare page with a `:param` is bare on every concrete path it resolves — what
/// [`crate::app::DeskApp`] reads to decide the chrome.
pub(crate) fn declared_bare_path(apps: &[AppUi], path: &str) -> bool {
    apps.iter().any(|app| {
        app.website.as_ref().is_some_and(|website| {
            website.routes.iter().any(|route| {
                route.kind.eq_ignore_ascii_case("bare") && route_matches(&route.path, path)
            })
        })
    })
}

/// `/blog/:slug` matches `/blog/hoy`: a `:param` segment matches any value,
/// every other segment must be byte-equal. Empty segments (leading, trailing
/// or doubled slashes) are insignificant.
fn route_matches(pattern: &str, path: &str) -> bool {
    fn segments(value: &str) -> Vec<&str> {
        value.split('/').filter(|part| !part.is_empty()).collect()
    }

    let declared = segments(pattern);
    let actual = segments(path);
    declared.len() == actual.len()
        && declared
            .iter()
            .zip(&actual)
            .all(|(param, value)| param.starts_with(':') || param == value)
}

/// A declared dashboard: header plus the 12-column grid of widget instances,
/// rendered by [`DashboardGrid`] — which loads the signed-in user's saved
/// override when one exists and offers the editor to users with the
/// `Dashboard Layout` create permission. Reached from the catch-all route and
/// from [`crate::views::HomeView`]'s `default` dashboard.
pub(crate) fn dashboard_view(owner: &str, dashboard: &Dashboard) -> AnyView {
    let label = if dashboard.label.is_empty() {
        dashboard.id.clone()
    } else {
        dashboard.label.clone()
    };
    view! {
        <DashboardGrid
            owner=owner.to_string()
            dashboard_id=dashboard.id.clone()
            label=label
            seed=dashboard.widgets.clone()
            declared_style=dashboard.style.clone()
        />
    }
    .into_any()
}

/// A declared website route: a full-page anchor the client fills with the
/// plugin's page component.
fn website_view(owner: &str, route: &WebsiteRoute) -> impl IntoView {
    let ctx = expect_context::<UiCtx>();
    let anchor: NodeRef<leptos::html::Div> = NodeRef::new();
    let mounted_owner = owner.to_string();
    let mounted_route = route.clone();

    Effect::new(move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            let apps = ctx.apps.get();
            crate::ext::loader::ensure_loaded(&apps);
            mount_website(anchor.get(), &mounted_owner, &mounted_route, &apps);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (anchor, ctx, &mounted_owner, &mounted_route);
    });

    view! {
        <div node_ref=anchor class="ferris-website-page" data-route=route.path.clone()></div>
    }
}

/// Mount a website page's component into its anchor's shadow root, replacing
/// the loading line once the plugin's script defines the element. F6 scopes the
/// page's markup and CSS to the shadow tree; only inherited properties (theme
/// custom properties included) cross into it.
#[cfg(target_arch = "wasm32")]
fn mount_website(
    anchor: Option<web_sys::HtmlDivElement>,
    owner: &str,
    route: &WebsiteRoute,
    apps: &[AppUi],
) {
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
    let tag = crate::ext::component_tag(&route.component);
    let defined = !registry.get(&tag).is_undefined();

    // The page's CSS, scoped to its shadow tree.
    let urls = crate::ext::loader::stylesheet_urls_for_owners(apps, &[owner]);
    crate::ext::loader::adopt(&shadow, &urls);

    // While the script travels, the page shows a line instead of a hole.
    if !defined && container.first_child().is_none() {
        container.set_inner_html(r#"<div class="text-sm text-muted-foreground">Cargando…</div>"#);
    }

    let route_path = route.path.clone();
    let component = route.component.clone();
    let owner = owner.to_string();

    crate::api::drive(async move {
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
            let _ = el.set_attribute("data-path", &route_path);
            let _ = el.set_attribute("data-component", &component);
            let _ = el.set_attribute("data-owner", &owner);
            let _ = container.append_child(el.as_ref());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext::{Website, WidgetInstance};

    fn instance(id: &str, widget: &str) -> WidgetInstance {
        WidgetInstance {
            id: id.to_string(),
            widget: widget.to_string(),
            ..Default::default()
        }
    }

    fn app(owner: &str) -> AppUi {
        AppUi {
            owner: owner.to_string(),
            ..Default::default()
        }
    }

    /// Website routes match by shape, not by string equality: `:params`
    /// swallow one segment each and the rest must line up exactly.
    #[test]
    fn website_route_patterns_match_by_segment() {
        assert!(route_matches("/status", "/status"));
        assert!(route_matches("/blog/:slug", "/blog/hoy"));
        assert!(route_matches("/status/", "/status"));
        assert!(!route_matches("/blog/:slug", "/blog/hoy/extra"));
        assert!(!route_matches("/blog/:slug", "/docs/hoy"));
        // Root matches root, not every path.
        assert!(route_matches("/", "/"));
        assert!(!route_matches("/", "/status"));
    }

    /// Only a dashboard whose `route` field is exactly the visited path
    /// matches; one with an empty route has no URL to catch.
    #[test]
    fn dashboards_are_found_by_their_declared_route() {
        let mut apps = vec![app("a")];
        apps[0].dashboards.push(Dashboard {
            id: "ops".to_string(),
            label: "Operaciones".to_string(),
            route: "/ops".to_string(),
            widgets: vec![instance("open", "open-stat")],
            ..Default::default()
        });
        apps[0].dashboards.push(Dashboard {
            id: "routless".to_string(),
            ..Default::default()
        });

        let found = find_dashboard(&apps, "/ops").expect("declared route resolves");
        assert_eq!(found.0, "a");
        assert_eq!(found.1.id, "ops");
        assert!(find_dashboard(&apps, "/nope").is_none());
        // The route-less dashboard is unreachable, not a catch-all.
        assert!(find_dashboard(&apps, "/routless").is_none());
        assert!(find_dashboard(&apps, "").is_none());
    }

    /// Website routes resolve to their owner app so the desk can load that
    /// app's assets; params and exact segments both work.
    #[test]
    fn website_routes_resolve_to_their_owner() {
        let mut apps = vec![app("site")];
        apps[0].website = Some(Website {
            routes: vec![WebsiteRoute {
                path: "/blog/:slug".to_string(),
                component: "blog-post".to_string(),
                kind: "page".to_string(),
            }],
        });

        let found = find_website_route(&apps, "/blog/hola").expect("param route resolves");
        assert_eq!(found.0, "site");
        assert_eq!(found.1.component, "blog-post");
        assert!(find_website_route(&apps, "/blog").is_none());
        assert!(find_website_route(&apps, "/nothing").is_none());
    }

    /// A website route declaring `kind: "bare"` is the only declaration that
    /// asks the Desk to leave the chrome out; a default `"page"` route at the
    /// same path keeps it. Matching is by shape, so a `:param` bare route is
    /// bare for every value it resolves.
    #[test]
    fn only_a_bare_website_route_asks_for_no_chrome() {
        let mut apps = vec![app("site")];
        apps[0].website = Some(Website {
            routes: vec![
                WebsiteRoute {
                    path: "/canvas".to_string(),
                    component: "canvas-app".to_string(),
                    kind: "bare".to_string(),
                },
                WebsiteRoute {
                    path: "/blog/:slug".to_string(),
                    component: "blog-post".to_string(),
                    kind: "page".to_string(),
                },
            ],
        });

        // Case-insensitive, and a trailing slash is insignificant.
        assert!(declared_bare_path(&apps, "/canvas"));
        assert!(declared_bare_path(&apps, "/canvas/"));
        // A declared page that did not opt out keeps the chrome.
        assert!(!declared_bare_path(&apps, "/blog/hoy"));
        // An undeclared path is never bare.
        assert!(!declared_bare_path(&apps, "/status"));
        assert!(!declared_bare_path(&[], "/canvas"));
    }

    /// The placeholder is what both SSR and the client's first pass render —
    /// before the feed arrives there is nothing to show and nothing for
    /// hydration to disagree about; once it lands the same path resolves to
    /// the declared dashboard.
    #[test]
    fn an_empty_feed_resolves_to_the_placeholder() {
        assert!(matches!(resolve_page(&[], "/ops"), Page::Placeholder));

        let mut apps = vec![app("a")];
        apps[0].dashboards.push(Dashboard {
            id: "ops".to_string(),
            route: "/ops".to_string(),
            ..Default::default()
        });
        assert!(matches!(resolve_page(&apps, "/ops"), Page::Dashboard(..)));
        assert!(matches!(resolve_page(&apps, "/otra"), Page::NotFound));
    }
}
