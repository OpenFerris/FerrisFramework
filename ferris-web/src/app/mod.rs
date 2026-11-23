//! Main Desk App component

use leptos::prelude::*;
use leptos::text_prop::TextProp;
use leptos_meta::*;
use leptos_router::hooks::{use_location, use_navigate};

use crate::WebUser;
// The session-refresh future and its driver only exist in the browser half of
// the shell; gating the imports keeps the native build free of unused ones.
#[cfg(target_arch = "wasm32")]
use crate::api::drive;
use crate::components::desk::top_bar::{
    TOP_BAR_OWNER_FORM, TopBar, TopBarMobileMenu, provide_top_bar_actions, provide_top_bar_chrome,
    provide_top_bar_doctype, provide_top_bar_owner, provide_top_bar_status, provide_top_bar_title,
    use_top_bar_chrome, use_top_bar_doctype, use_top_bar_owner,
};
use crate::components::hooks::use_is_mobile::use_is_mobile;
use crate::components::hooks::use_theme_mode::ThemeMode;
use crate::components::ui::toast_custom::toaster::{Toaster, provide_toaster};
use crate::desk::relogin::ReloginDialog;
#[cfg(target_arch = "wasm32")]
use crate::desk::session as desk_session;
use crate::desk::session::{DeskIcon, NotificationFeed, NotificationSettings, SiteSettings};
use crate::desk::{DeskTopBar, GlobalSearch, MobileNav, NotificationsPanel, Sidebar};
use crate::ext::{
    SLOT_FORM_TOOLBAR_LEFT, SLOT_FORM_TOOLBAR_RIGHT, SLOT_SHELL_TOPBAR_RIGHT, UiCtx, UiSlot,
};
use crate::theme::ThemeCtx;

use self::announcement::Announcement;
use self::mobile::{MobileSidebarDrawer, bottom_nav_visible, unread_count};

mod announcement;
mod breadcrumbs;
pub mod mobile;
mod routes;
mod state;

use self::routes::DeskRoutes;

pub use mobile::{MobilePrefs, MobilePrefsCtx, provide_mobile_prefs, use_mobile_prefs};
pub(crate) use state::refresh_notifications;
pub use state::{Workspace, WorkspaceItem, WorkspaceItemKind};
use state::{desk_title, page_wants_bare_chrome, refresh_site, refresh_workspaces};

/// The desk page is the whole page: it carries the grid itself, so the sidebar
/// and everything that lives inside it are left out rather than hidden behind a
/// collapsed rail.
pub const DESK_PATH: &str = "/desk";

/// The standalone sign-in route. The server redirects an unauthenticated desk
/// request here (with `?next=`), and the [`LoginPage`] route renders it without
/// any of the desk chrome.
pub const LOGIN_PATH: &str = "/login";

/// What a successful sign-in has to do to the desk before the login page hands
/// control back: adopt the identity, load the workspace grid and the
/// notification feed, and refresh the OpenUI manifest for the new user.
///
/// Provided as context because the `/login` route is instantiated by `<Routes>`
/// with no props, so [`LoginPage`] cannot be handed the desk's own signals.
#[derive(Clone, Copy)]
pub struct LoginComplete(pub Callback<WebUser>);

/// Whether the current page renders without the desk chrome (sidebar and top
/// bar).
///
/// It is a [`Memo`] — a pure function of the route and the apps' declared pages
/// — rather than a signal a view writes, so there is one source of truth and
/// SSR cannot disagree with hydration.
#[derive(Clone, Copy)]
pub struct BareChrome(pub Memo<bool>);

/// The install-wide presentation settings, provided to any view that needs to
/// show them (the settings page) without a second `/api/site-settings` fetch.
#[derive(Clone, Copy)]
pub struct SiteState(pub RwSignal<SiteSettings>);

/// Read the site settings signal, or a never-updated default when no shell is
/// in scope (standalone view tests, plugin pages rendered outside the Desk).
pub fn use_site_state() -> RwSignal<SiteSettings> {
    use_context::<SiteState>()
        .map_or_else(|| RwSignal::new(SiteSettings::default()), |state| state.0)
}

/// Provide the bare-chrome answer so any view can read it.
pub fn provide_bare_chrome(value: Memo<bool>) {
    provide_context(BareChrome(value));
}

/// Read the bare-chrome answer (`false` when no provider is in scope).
pub fn use_bare_chrome() -> Memo<bool> {
    use_context::<BareChrome>().map_or_else(|| Memo::new(|_| false), |ctx| ctx.0)
}

/// What the desk's own views need from [`DeskApp`]: the workspaces the sidebar
/// draws, the arranged grid the desk page draws, and how to select a workspace.
///
/// Provided as context because `<Routes>` instantiates a view with no props, so
/// the desk grid cannot be handed them by its route entry.
#[derive(Clone, Copy)]
pub struct DeskNav {
    pub workspaces: RwSignal<Vec<Workspace>>,
    pub icons: RwSignal<Vec<DeskIcon>>,
    pub select: Callback<String>,
}

/// Main desk application.
#[component]
pub fn DeskApp() -> impl IntoView {
    provide_meta_context();

    // The notification toaster lives on the Desk shell: the live stream raises
    // toasts, and every future in-app message can use the same one.
    provide_toaster();

    // Per-device presentation preferences (bottom bar, sidebar drawer, the
    // form's own sidebar); the stored values are applied after hydration.
    let mobile_prefs = provide_mobile_prefs();
    let is_mobile = use_is_mobile();

    let user = expect_context::<RwSignal<WebUser>>();
    let location = use_location();
    let breadcrumb_i18n = StoredValue::new(crate::i18n::use_i18n());

    let (sidebar_open, set_sidebar_open) = signal(true);
    // The mobile drawer's own visibility. Kept apart from `sidebar_open` so
    // opening the drawer on a phone never folds the rail a wide window shows.
    let mobile_menu_open = RwSignal::new(false);
    // Both mobile bars read this to offer the drawer; the preference can hide
    // the button instead of leaving one that does nothing.
    provide_context(TopBarMobileMenu {
        open: mobile_menu_open,
        available: Signal::derive(move || mobile_prefs.get().sidebar_drawer),
    });

    // Theme state: provided so the TopBar selector and any future consumer can
    // read the available themes and switch the active one.
    let theme_ctx = ThemeCtx::default_ctx();
    provide_context(theme_ctx.clone());

    // OpenUI slot registry: apps' declared extensions, fetched on the client.
    let ui_ctx = UiCtx::default_ctx();
    provide_context(ui_ctx);

    // Dark-mode state: `init` provides it, so every `use_theme_mode` caller
    // (the sidebar toggle, the top bar) finds it.
    let _ = ThemeMode::init();

    // Reactively-loaded desk state (P0/P1/P2).
    let workspaces = RwSignal::new(Vec::<Workspace>::new());
    // The arranged grid, in the order it is drawn. Owned here rather than inside
    // the grid so a save that comes back from the server lands everywhere at
    // once, and so the drag-and-drop edits and the fetch that seeds them write
    // to the same place.
    let icons = RwSignal::new(Vec::<DeskIcon>::new());
    // The workspace the user last picked. The route is what actually decides it
    // while it names a workspace (see `Sidebar`); this keeps the sidebar on the
    // chosen workspace for the routes that belong to none (`/`, `/desk`, …).
    let picked_workspace = RwSignal::new(None::<String>);
    let notifications = RwSignal::new(NotificationFeed::default());
    // Install-wide presentation settings (`/api/site-settings`): the tab-title
    // prefix and the navbar announcement/help link, driven by the `Website
    // Settings` and `Navbar Settings` Singles rather than hardcoded strings.
    let site = RwSignal::new(SiteSettings::default());
    provide_context(SiteState(site));
    // Owned here, not inside the panel: the unread badge lives in the sidebar,
    // so it needs the `desktop` preference to decide whether to draw.
    let notif_settings = RwSignal::new(NotificationSettings::default());
    let search_open = RwSignal::new(false);
    let search_query = RwSignal::new(String::new());
    let notifications_open = RwSignal::new(false);
    // What the chrome shows as unread: the rail's badge, the bar's dot and the
    // tab title all read this one count, and the "desktop notifications"
    // preference turns it off for every surface at once.
    let unread = unread_count(notifications, notif_settings);

    // Contextual top-bar actions: the active view publishes into this registry
    // (see `use_top_bar_actions`) and the top bar renders it.
    provide_top_bar_actions();
    // The optional status indicator beside the breadcrumbs (the form's "No
    // guardado" / "Enviado"): only the form publishes one, and it clears it on
    // teardown, so every other route keeps the plain breadcrumbs.
    provide_top_bar_status();
    // The optional page title drawn between the breadcrumbs and the status
    // (the dashboard builder's board name). Same contract as the status: the
    // owning view publishes and clears it on teardown, every other route
    // renders none.
    provide_top_bar_title();
    // Which view owns the bar (`None` for every view that has no view-scoped
    // anchors). Only the form writes it, so reading it needs no race handling.
    provide_top_bar_owner();
    // The doctype that owner is showing. Also view-owned: the bar renders as a
    // sibling of `<Routes>`, so it cannot read the route's `:doctype` param
    // itself (`use_params_map()` panics outside a matched `<Route>`).
    provide_top_bar_doctype();
    // How the bar lays itself out: a dashboard can enter full-screen mode and
    // decide whether its title survives it. Every other view leaves the plain
    // chrome; the shell reads the flag to hide the sidebar.
    provide_top_bar_chrome();

    // Whether the current page draws its own surface. Derived, not set: the
    // route decides for `/login` and the desk grid, and an app's declared
    // website page decides for itself with `kind: "bare"` (see
    // `page_wants_bare_chrome`). Computing it — instead of an effect writing a
    // flag — is what keeps SSR and hydration identical: both start with an
    // empty UI feed, so a declared bare page only hides the chrome once the
    // client feed lands, exactly like every other feed-driven element.
    let bare_chrome =
        Memo::new(move |_| page_wants_bare_chrome(&ui_ctx.apps.get(), &location.pathname.get()));
    provide_bare_chrome(bare_chrome);
    // `/desk` is deliberately bare — no sidebar — but it still deserves the
    // chrome the other views get: search, notifications and the user menu. It
    // is a route test of its own rather than `!bare_chrome`, because the app's
    // declared offline pages are bare too and must stay unadorned.
    let desk_home = Memo::new(move |_| location.pathname.get().trim_end_matches('/') == DESK_PATH);

    // Selecting a workspace pins it for the routes that belong to no workspace
    // and opens its first link. Built here, in the component body, so the
    // `Callback`'s value is owned by this owner: one created inside a re-running
    // reactive closure would be disposed the next time that closure runs.
    let navigate = use_navigate();
    let select_workspace = Callback::new(move |name: String| {
        picked_workspace.set(Some(name.clone()));
        let route = workspaces.with_untracked(|list| {
            list.iter()
                .find(|ws| ws.name == name)
                .and_then(Workspace::first_route)
        });
        if let Some(route) = route {
            navigate(&route, leptos_router::NavigateOptions::default());
        }
    });
    // The header's workspace links navigate to the clicked link themselves, so
    // picking a workspace there must only pin it — navigating to the first link
    // here would override the link the user actually chose.
    let pick_workspace = Callback::new(move |name: String| {
        picked_workspace.set(Some(name));
    });
    provide_context(DeskNav {
        workspaces,
        icons,
        select: select_workspace,
    });

    // The form-scoped OpenUI anchors (`form.toolbar.left`/`right`) moved from
    // the form's own header into the shared top bar when the form stopped
    // drawing a second bar. Both the owner and the doctype come from the view
    // that owns the bar (see `TopBarDoctype`): this component is the sibling of
    // `<Routes>`, so it has no matched route to read the `:doctype` param from.
    let top_bar_owner = use_top_bar_owner();
    let form_chrome = Signal::derive(move || top_bar_owner.get() == Some(TOP_BAR_OWNER_FORM));
    let form_doctype = use_top_bar_doctype();
    let slot_doctype = Signal::derive(move || form_doctype.get());

    // Bootstrap the session once on the client (SSR keeps rendering the guest
    // state so hydration stays deterministic).
    #[cfg(target_arch = "wasm32")]
    {
        use crate::desk::notifications;
        use crate::desk::session::Notification;

        let user = user;
        let workspaces = workspaces;
        let icons = icons;
        let notifications = notifications;
        let notif_settings = notif_settings.clone();
        let site = site.clone();

        // Refresh the bootstrap meta whenever the SPA identity changes, so a
        // plugin reading `meta[name="ferris-user"]` sees the same user as the
        // sidebar after login/logout (there is no new SSR document to restamp).
        Effect::new(move |_| crate::shell::stamp_bootstrap_user(&user.get()));

        // `/login` is served to a guest, and nothing on that page needs the
        // desk's app manifest or a session probe: all three would answer 401
        // and add console noise to every anonymous visit. Restore the saved
        // palette locally instead; the full boot runs after a successful
        // sign-in (`on_login`) or on any authenticated page.
        let guest_on_login = user.get_untracked().is_guest()
            && location.pathname.get_untracked() == crate::app::LOGIN_PATH;
        if guest_on_login {
            theme_ctx.init_local();
        } else {
            // Restore the persisted theme and fetch any app-registered themes.
            theme_ctx.init();

            // Fetch the apps' declared OpenUI extensions (slots, dashboards,
            // website routes) once, so `<UiSlot>` anchors can fill themselves.
            ui_ctx.init();
        }

        let listener = window_event_listener(leptos::ev::keydown, move |event| {
            if (event.ctrl_key() || event.meta_key()) && event.key().to_lowercase() == "k" {
                event.prevent_default();
                search_open.set(!search_open.get());
            }
        });
        let _ = listener;

        // Whether SSR had already resolved a session. A desk page is only ever
        // rendered authenticated, so this is what tells a genuine mid-hydration
        // expiry apart from the normal "guest on `/login`" answer.
        let boot_authenticated = !user.get_untracked().is_guest();

        // Where a lost session sends the browser. `/login` is its own page, so
        // a guest is never left on a desk that would only fail to load.
        let go_login = use_navigate();
        let pathname = location.pathname;
        let search = location.search;

        if !guest_on_login {
            drive(async move {
                match desk_session::me().await {
                    Ok(Some(me)) => {
                        // Adopt the identity when SSR only knew a guest; never
                        // clobber a fresher one another tab already set.
                        if user.get_untracked().is_guest() {
                            user.set(me);
                        }
                        // The desk's own data is client-loaded: SSR renders the
                        // identity but not the grid or the feed, so seed them here
                        // whether the identity came from the `<meta>` bootstrap or
                        // from this call. `me()` resolving is what confirms the
                        // session is live — otherwise a signed-in reload shows an
                        // empty desk (no workspaces, no icons, no notifications).
                        refresh_workspaces(workspaces, icons);
                        refresh_notifications(notifications, notif_settings);
                        refresh_site(site);
                    }
                    Ok(None) => {
                        // Booting as a guest (i.e. on `/login`) answers `None`
                        // normally. A previously-authenticated load that now gets
                        // `None` has actually lost its session. Bailing early also
                        // protects a login that landed while this call was still in
                        // flight: `user` was a guest at boot, so the fresh session
                        // is never redirected away.
                        if !boot_authenticated {
                            return;
                        }
                        user.set(WebUser::guest());
                        // A desk page is only ever served to a session that existed
                        // when it was rendered. Reaching here means the session
                        // expired (or was revoked) between render and hydration:
                        // send the browser to /login with the page it asked for
                        // instead of leaving it on a desk that cannot load
                        // anything.
                        let path = pathname.get_untracked();
                        if path != LOGIN_PATH {
                            let query = search.get_untracked();
                            let target = if query.is_empty() {
                                path
                            } else {
                                format!("{path}?{query}")
                            };
                            go_login(&state::login_redirect(&target), Default::default());
                        }
                    }
                    Err(_) => {}
                }
            });
        }

        // Live notifications: the stream fills `feed`, queues arrivals here, and
        // this effect drains them so the toast's click callback is built inside
        // the component's owner — a DOM message handler runs outside one.
        let navigate = use_navigate();
        let incoming = RwSignal::new(Vec::<Notification>::new());
        let announce_toaster = crate::components::ui::toast_custom::toaster::expect_toaster();

        Effect::new(move |_| {
            let pending = incoming.get();
            if pending.is_empty() {
                return;
            }
            incoming.set(Vec::new());
            for item in pending {
                notifications::announce(
                    item,
                    notifications,
                    notifications_open,
                    announce_toaster.clone(),
                    navigate.clone(),
                );
            }
        });

        // One stream per signed-in session: open it on sign-in, drop it on
        // sign-out so a guest never holds an authenticated subscription.
        Effect::new(move |_| {
            if user.get().is_guest() {
                notifications::stop();
            } else {
                notifications::start(notifications, incoming);
            }
        });
    }

    // What `/login` runs on success. Handed to the route through context (not
    // props) because `<Routes>` instantiates its views with no arguments. The
    // navigation itself stays in `LoginPage`, which is the only place that
    // knows the `?next=` destination.
    let on_login = Callback::new(move |me: WebUser| {
        user.set(me);
        refresh_workspaces(workspaces, icons);
        refresh_notifications(notifications, notif_settings);
        refresh_site(site);
        // Fase 4: the feed may be session-scoped from here on (and may have
        // moved while the login screen was up): refetch so the slots render
        // the just-logged-in user's manifest.
        ui_ctx.refresh();
        theme_ctx.refresh();
    });
    provide_context(LoginComplete(on_login));

    // `/login` is its own page: an unauthenticated desk request is redirected
    // here by the server, and the desk chrome is left out rather than drawn
    // underneath a facade. `<Routes/>` stays unconditional so the router's
    // `generate_route_list` records every deep link (a direct request to
    // /desk/* is answered by the SSR server, not a 404). `bare_chrome` already
    // covers `/login`, the desk grid and any app-declared bare page.

    // The tab title. On the server `leptos_meta` reads the `TextProp` back
    // *after* the SSR arena has been torn down, so a reactive closure there
    // would touch arena-backed signals with no arena active and panic under
    // `sandboxed-arenas`. Resolve it eagerly on the server; keep the closure on
    // the client, where `Title` drives it from a `RenderEffect` and the unread
    // count stays live.
    let title = if cfg!(target_arch = "wasm32") {
        TextProp::from(move || desk_title(notifications, notif_settings, site))
    } else {
        TextProp::from(desk_title(notifications, notif_settings, site))
    };

    let top_bar_chrome = use_top_bar_chrome();
    // Dashboard full screen: the board owns the window, so the shell drops the
    // sidebar and leaves the bar as the toggle plus (maybe) the title. The bar
    // itself filters its actions; the shell only removes the navigation.
    let fullscreen = Signal::derive(move || top_bar_chrome.get().fullscreen);

    // A board in full-screen mode owns the viewport: a drawer or a notification
    // panel left open from the previous chrome would float over it.
    Effect::new(move |_| {
        if fullscreen.get() {
            mobile_menu_open.set(false);
            notifications_open.set(false);
        }
    });

    // Whether the bottom bar is on screen. `<main>` reserves its height too, so
    // no view's last row hides behind it.
    let bottom_nav = bottom_nav_visible(
        is_mobile,
        mobile_prefs,
        bare_chrome,
        location.pathname,
        fullscreen,
    );

    // A closure builds a fresh bar whenever the shell re-renders — a `view!`
    // value cannot be mounted twice.
    let top_bar = move || {
        view! {
            <TopBar
                // The app builds the trail; the signal keeps it following navigation.
                breadcrumbs=Signal::derive(move || {
                    breadcrumb_i18n.with_value(|i| breadcrumbs::trail(&location.pathname.get(), i))
                })
                leading=view! {
                    <Show when=move || form_chrome.get()>
                        <UiSlot slot_name=SLOT_FORM_TOOLBAR_LEFT doctype=slot_doctype />
                    </Show>
                }
                .into_any()
                trailing=view! {
                    <Show when=move || form_chrome.get()>
                        <UiSlot slot_name=SLOT_FORM_TOOLBAR_RIGHT doctype=slot_doctype />
                    </Show>
                    <UiSlot slot_name=SLOT_SHELL_TOPBAR_RIGHT />
                }
                .into_any()
            />
        }
    };

    view! {
        <Meta name="viewport" content="width=device-width, initial-scale=1" />
        <Meta charset="utf-8" />
        <Title text=title />

        <div class="desk-app h-dvh flex flex-col bg-muted">
            <div class="flex min-h-0 flex-1">
                {move || {
                    if bare_chrome.get() || fullscreen.get() {
                        return view! { <span></span> }.into_any();
                    }
                    // CSS decides which sidebar a viewport gets: the rail is
                    // hidden below `md` and the drawer takes over. Gating this
                    // on `is_mobile` in Rust made the server paint the desktop
                    // rail on a phone and wasm remove it after hydration — a
                    // visible layout jump on every mobile load. The drawer hides
                    // itself above `md` too, so a stale open flag cannot leave it
                    // over a wide window.
                    view! {
                        <div class="hidden h-full md:flex">
                            <Sidebar
                                workspaces=workspaces.read_only()
                                picked=picked_workspace.read_only()
                                open=sidebar_open
                                on_toggle=set_sidebar_open
                                on_pin=pick_workspace
                                on_search=move || search_open.set(true)
                                on_notifications=move || notifications_open.update(|o| *o = !*o)
                                unread=unread
                                current_route=location.pathname
                            />
                        </div>
                    }
                    .into_any()
                }}
                // The drawer and the notification panel must exist on `/desk`
                // (no rail, but a mobile bottom bar) and on every ordinary page.
                // They must NOT exist on `/login` or another bare page: those
                // pages have no navigation, and mounting them there was what
                // broke hydration (a panel the server tree does not have).
                {move || {
                    let overlays = !fullscreen.get()
                        && (!bare_chrome.get() || desk_home.get());
                    if !overlays {
                        return AnyView::from(Fragment::new(Vec::new()));
                    }
                    view! {
                        <MobileSidebarDrawer
                            workspaces=workspaces.read_only()
                            picked=picked_workspace.read_only()
                            open=mobile_menu_open
                            on_pin=pick_workspace
                            search_open=search_open
                            notifications_open=notifications_open
                            unread=unread
                            current_route=location.pathname
                        />
                        <NotificationsPanel
                            open=notifications_open
                            feed=notifications
                            settings=notif_settings
                        />
                    }
                    .into_any()
                }}
                <main
                    class=move || {
                        if bottom_nav.get() {
                            "flex-1 flex flex-col overflow-hidden pb-[calc(var(--bottom__nav__height,4rem)_+_env(safe-area-inset-bottom,0px))]"
                        } else {
                            "flex-1 flex flex-col overflow-hidden"
                        }
                    }
                >
                    {move || (!bare_chrome.get()).then(top_bar)}
                    {move || desk_home.get().then(|| view! {
                        <DeskTopBar
                            user=user
                            search_open=search_open
                            notifications=notifications
                            settings=notif_settings
                        />
                    })}
                    <Announcement bare_chrome=bare_chrome fullscreen=fullscreen />
                    // The content surface owns no padding: each view decides
                    // its own spacing (the dashboard fills the area, the form
                    // and the custom pages lay themselves out). A fixed `p-4`
                    // here used to add a gutter every view had to fight.
                    <div class="flex-1 overflow-auto">
                        <DeskRoutes />
                    </div>
                </main>
            </div>
        </div>

        <GlobalSearch open=search_open query=search_query />
        // Self-gating (mobile viewport, preference, desk chrome): the bar's rule
        // lives in one place instead of being re-stated by every host.
        <MobileNav
            search_open=search_open
            notifications_open=notifications_open
            menu_open=mobile_menu_open
            unread=unread
        />
        // One session-expiry modal for the whole shell: the transport raises its
        // global flag, and this is the only reader. Mounting it here (not per
        // view) is what keeps the form mounted underneath when the session is
        // renewed.
        <ReloginDialog />
        <Toaster />
    }
}
