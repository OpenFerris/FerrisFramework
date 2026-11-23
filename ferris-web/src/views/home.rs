//! Fase 2: `/` deja de ser la landing Rust hardcoded → [`HomeView`].
//!
//! Si alguna app declaró un dashboard con `default: true`, `/` lo renderiza
//! (el registro garantiza a lo sumo uno — el primero gana, con warning, en
//! `ferris_core::ui_extensions`); si no, un **índice** con los dashboards
//! declarados: tarjetas con label/ruta/owner que enlazan a sus rutas.
//!
//! Mismo contrato de hidratación que [`crate::ext::UiSlot`] y
//! [`crate::views::PluginPage`]: SSR no tiene feed, así que ambos lados
//! pintan el mismo esqueleto y el cliente lo rellena cuando llega.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::components::ui::icon::Icon;
use crate::ext::{AppUi, Dashboard, UiCtx};
use crate::i18n::use_i18n;
use crate::views::plugin_page::{dashboard_view, feed_error, placeholder};
use crate::views::views_components::{EmptyState, PageHeader};

/// The desk's `/`: the feed's default declared dashboard, else its index.
#[component]
pub fn HomeView() -> impl IntoView {
    let ctx = expect_context::<UiCtx>();
    let i18n = use_i18n();

    view! {
        <div class="ferris-home h-full overflow-y-auto p-4">
            {move || {
                let apps = ctx.apps.get();
                if apps.is_empty() {
                    // An empty feed is "still loading" only until it fails; then
                    // it is an error with a retry, not an endless skeleton.
                    return ctx
                        .error
                        .get()
                        .map_or_else(placeholder, |message| feed_error(ctx, message, i18n));
                }
                match default_dashboard(&apps) {
                    Some((owner, dashboard)) => dashboard_view(owner, dashboard),
                    None => dashboard_index(&apps, &i18n),
                }
            }}
        </div>
    }
}

/// The first dashboard registered with `default: true` — the server keeps
/// that flag to at most one (a later claim is dropped with a warning), so
/// "first wins" is the whole rule here.
fn default_dashboard(apps: &[AppUi]) -> Option<(&str, &Dashboard)> {
    apps.iter().find_map(|app| {
        app.dashboards
            .iter()
            .find(|dashboard| dashboard.default)
            .map(|dashboard| (app.owner.as_str(), dashboard))
    })
}

/// Nothing declared `default`: every dashboard in the feed, as cards
/// linking to their declared routes.
fn dashboard_index(apps: &[AppUi], i18n: &(impl crate::i18n::Translate + Sync)) -> AnyView {
    let cards: Vec<(&str, &Dashboard)> = apps
        .iter()
        .flat_map(|app| {
            app.dashboards
                .iter()
                // A dashboard without a route has no page to link to: the
                // index is a list of destinations, so a routeless board would
                // be a card whose link goes nowhere.
                .filter(|dashboard| !dashboard.route.trim().is_empty())
                .map(move |dashboard| (app.owner.as_str(), dashboard))
        })
        .collect();
    let declared = apps.iter().any(|app| !app.dashboards.is_empty());

    if cards.is_empty() {
        return view! {
            <EmptyState
                icon="LayoutDashboard"
                title=if declared {
                    i18n.t("views.home.no_route_title")
                } else {
                    i18n.t("views.home.empty_title")
                }
                description=if declared {
                    i18n.t("views.home.no_route_description")
                } else {
                    i18n.t("views.home.empty_description")
                }
            />
        }
        .into_any();
    }

    view! {
        <section class="mx-auto flex w-full max-w-7xl flex-col gap-4">
            <PageHeader
                title=i18n.t("views.home.title")
                description=i18n.t("views.home.description")
            />
            <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
                {cards
                    .into_iter()
                    .map(|(owner, dashboard)| {
                        // The view tree stores its dynamic children as
                        // `'static` closures, so every borrowed value the
                        // card shows is owned before the `view!` starts.
                        let title = if dashboard.label.is_empty() {
                            dashboard.id.clone()
                        } else {
                            dashboard.label.clone()
                        };
                        let route = dashboard.route.clone();
                        let href = route.clone();
                        let owner = owner.to_string();
                        // The registry already demotes a second `default`,
                        // so the badge only ever marks the winner.
                        let badge = dashboard.default.then(|| i18n.t("views.home.default_badge"));
                        view! {
                            <A
                                href=href
                                attr:class="group flex flex-col gap-3 rounded-xl border border-border bg-card p-4 shadow-sm transition-colors hover:border-ring/60 hover:bg-accent/40 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 sm:p-6"
                            >
                                <div class="flex items-start justify-between gap-3">
                                    <span class="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
                                        <Icon name="LayoutDashboard" class="size-4" />
                                    </span>
                                    {badge.map(|text| view! {
                                        <Badge variant=BadgeVariant::Secondary size=BadgeSize::Sm>{text}</Badge>
                                    })}
                                </div>
                                <div class="flex min-w-0 flex-col gap-1">
                                    <span class="truncate text-sm font-medium text-foreground">{title}</span>
                                    <span class="truncate text-xs text-muted-foreground">
                                        <code>{route}</code>
                                        " · "{owner}
                                    </span>
                                </div>
                            </A>
                        }
                    })
                    .collect_view()}
            </div>
        </section>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(owner: &str) -> AppUi {
        AppUi {
            owner: owner.to_string(),
            ..Default::default()
        }
    }

    fn dashboard(id: &str, route: &str, default: bool) -> Dashboard {
        Dashboard {
            id: id.to_string(),
            label: id.to_string(),
            route: route.to_string(),
            default,
            ..Default::default()
        }
    }

    /// `default` is server-kept to a single claim (first registered wins);
    /// the view's job is only to pick that first `default: true`.
    #[test]
    fn the_first_default_dashboard_wins_and_none_falls_through_to_the_index() {
        let mut first = app("one");
        first.dashboards.push(dashboard("landing", "/one", false));
        first.dashboards.push(dashboard("home", "/home", true));
        let mut second = app("two");
        second
            .dashboards
            .push(dashboard("other-home", "/two-home", true));
        let apps = vec![first, second];

        let found = default_dashboard(&apps).expect("a default is declared");
        assert_eq!(found.0, "one");
        assert_eq!(found.1.id, "home");

        let no_default = vec![{
            let mut plain = app("plain");
            plain.dashboards.push(dashboard("only", "/only", false));
            plain
        }];
        assert!(default_dashboard(&no_default).is_none());
    }

    /// Apps in the feed with no dashboards at all fall through to the index
    /// (its "empty" panel), never to a dashboard render — and an empty feed
    /// is the placeholder's case, shared by SSR and the client's first pass.
    #[test]
    fn apps_without_dashboards_never_render_a_default() {
        let apps = vec![app("bare"), app("empty")];
        assert!(default_dashboard(&apps).is_none());
        assert!(default_dashboard(&[]).is_none());
    }
}
