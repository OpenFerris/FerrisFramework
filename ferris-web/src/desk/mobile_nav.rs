//! The mobile bottom navigation bar.
//!
//! On a phone the rail is a drawer that has to be opened, so the four things a
//! small-screen user needs constantly — the desk, search, notifications and the
//! menu — get a thumb-reachable bar instead. It is rendered only below the
//! mobile breakpoint and only while the device preference keeps it on (see
//! [`crate::app::mobile`]); a desktop tab never draws it, even after the bar is
//! switched off and on again.
//!
//! The bar is pinned to the bottom of the viewport and pads itself for the safe
//! area (`BottomNav` owns that), so the shell only has to reserve the height —
//! it does that on `<main>` in `app::DeskApp`.

use leptos::prelude::*;
use leptos_router::hooks::{use_location, use_navigate};

use crate::app::DESK_PATH;
use crate::app::{use_bare_chrome, use_mobile_prefs};
use crate::components::desk::top_bar::use_top_bar_chrome;
use crate::components::ui::bottom_nav::{
    BottomNav, BottomNavButton, BottomNavGrid, BottomNavLabel,
};
use crate::components::ui::icon::Icon;
use crate::i18n::{DeskI18n, use_i18n};

/// Bottom navigation: `Escritorio`, `Buscar`, `Avisos` and `Menú`.
#[component]
pub fn MobileNav(
    /// Opens the command palette.
    search_open: RwSignal<bool>,
    /// Toggles the docked notifications panel.
    notifications_open: RwSignal<bool>,
    /// Toggles the sidebar drawer (a separate signal from the desktop rail).
    menu_open: RwSignal<bool>,
    /// How many notifications are unread; a dot is drawn when it is not zero.
    unread: Memo<u64>,
) -> impl IntoView {
    let prefs = use_mobile_prefs();
    // `StoredValue` keeps the translator `Copy`, so the reactive fallback and
    // the static labels can share it.
    let i18n = use_i18n();
    let tr = move |key: &str| i18n.with_value(|i18n: &DeskI18n| i18n.t(key));
    // `/login` and app-declared bare pages own the whole page: no bar there.
    let bare_chrome = use_bare_chrome();
    // A full-screen board owns the viewport, bar included.
    let chrome = use_top_bar_chrome();
    let fullscreen = Signal::derive(move || chrome.get().fullscreen);
    let pathname = use_location().pathname;
    let navigate = {
        let navigate_fn = use_navigate();
        // A `Callback`, not the router handle itself: the handle is not `Copy`,
        // and two closures cannot both capture it — nor can a children closure
        // that moves it into an event handler stay `Fn`. `Callback` is `Copy`.
        Callback::new(move |route: &'static str| {
            navigate_fn(route, leptos_router::NavigateOptions::default());
        })
    };

    // `/desk` is the bar's own destination; the trailing slash the router may
    // leave behind must not unset the active state.
    let desk_active = move || pathname.get().trim_end_matches('/') == DESK_PATH;
    let go_desk = Callback::new(move |()| navigate.run(DESK_PATH));
    // With the drawer switched off, the slot becomes the way to the settings
    // page that switched it off: leaving the bar with three items and no
    // reachable settings screen is how a "hide this" preference locks itself.
    let go_settings = Callback::new(move |()| navigate.run("/settings"));

    view! {
        // The viewport decision is CSS (`md:hidden`), not a Rust media-query
        // signal: the server markup then already carries the bar a phone needs,
        // instead of adding it after hydration. A wide window never shows it.
        <Show when=move || {
            let is_desk = pathname.get().trim_end_matches('/') == DESK_PATH;
            prefs.get().bottom_nav && !fullscreen.get() && (!bare_chrome.get() || is_desk)
        }>
            <BottomNav
                class="fixed inset-x-0 bottom-0 md:hidden"
                attr:aria-label=tr("desk.nav.label")
            >
                <BottomNavGrid>
                    <BottomNavButton
                        attr:aria-label=tr("nav.desk")
                        attr:aria-current=move || if desk_active() { "page" } else { "false" }
                        on:click=move |_| go_desk.run(())
                    >
                        <Icon name="LayoutGrid" class="size-5" />
                        <BottomNavLabel>{tr("nav.desk")}</BottomNavLabel>
                    </BottomNavButton>

                    <BottomNavButton
                        attr:aria-label=tr("desk.nav.search")
                        on:click=move |_| search_open.set(true)
                    >
                        <Icon name="Search" class="size-5" />
                        <BottomNavLabel>{tr("desk.nav.search")}</BottomNavLabel>
                    </BottomNavButton>

                    <BottomNavButton
                        class="relative"
                        attr:aria-label=tr("desk.nav.notifications")
                        attr:aria-expanded=move || notifications_open.get().to_string()
                        on:click=move |_| notifications_open.update(|open| *open = !*open)
                    >
                        <Icon name="Bell" class="size-5" />
                        // The same dot the rail draws, and the same token: the
                        // label and the icon already come from the bar's own
                        // `aria-[current=page]` state, so the dot is only a
                        // second signal, never the only one.
                        <Show when=move || { unread.get() > 0 }>
                            <span
                                class="absolute right-1.5 top-1 h-1.5 w-1.5 rounded-full bg-primary"
                                aria-hidden="true"
                            ></span>
                        </Show>
                        <BottomNavLabel>{tr("desk.nav.notifications")}</BottomNavLabel>
                    </BottomNavButton>

                    // Without the drawer the rail is unreachable on a phone, so
                    // the slot becomes the settings shortcut instead of a
                    // control that does nothing (or a bar missing its way back).
                    <Show
                        when=move || prefs.get().sidebar_drawer
                        fallback=move || view! {
                            <BottomNavButton
                                attr:aria-label=tr("nav.settings")
                                on:click=move |_| go_settings.run(())
                            >
                                <Icon name="Settings" class="size-5" />
                                <BottomNavLabel>{tr("nav.settings")}</BottomNavLabel>
                            </BottomNavButton>
                        }
                    >
                        <BottomNavButton
                            attr:aria-label=tr("desk.nav.menu")
                            attr:aria-expanded=move || menu_open.get().to_string()
                            on:click=move |_| menu_open.update(|open| *open = !*open)
                        >
                            <Icon name="Menu" class="size-5" />
                            <BottomNavLabel>{tr("desk.nav.menu")}</BottomNavLabel>
                        </BottomNavButton>
                    </Show>
                </BottomNavGrid>
            </BottomNav>
        </Show>
    }
}
