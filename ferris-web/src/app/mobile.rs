//! Per-device mobile preferences, and the drawer the shell uses on small screens.
//!
//! The Desk has one layout per viewport: a 16rem rail on a desktop and, on a
//! phone, a bottom bar plus an on-demand drawer. Which of those a *device* shows
//! is not an install-wide setting — it belongs to the browser that is looking at
//! the page — so these preferences live in `localStorage` next to the theme, not
//! in `/api/site-settings`.
//!
//! Storage is read and written in effects rather than in the component body on
//! purpose: SSR renders the defaults and the hydration pass must produce the same
//! tree before the stored values are applied. Doing it eagerly would make the
//! first client render disagree with the server's markup.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::app::Workspace;
use crate::desk::Sidebar;
use crate::desk::session::{NotificationFeed, NotificationSettings};
use crate::i18n::use_i18n;

/// Where the per-device preferences are stored.
pub const MOBILE_PREFS_STORAGE_KEY: &str = "ferris.mobile.preferences";

/// How the Desk should present itself on a small screen.
///
/// `#[serde(default)]` keeps a value stored by an older build readable: a
/// preference added later is simply the default until the user touches it, and
/// an older key never invalidates the whole object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MobilePrefs {
    /// Draw the bottom navigation bar.
    pub bottom_nav: bool,
    /// Offer the sidebar as a drawer instead of hiding it outright.
    pub sidebar_drawer: bool,
    /// Show the form's right sidebar on mobile (hidden by default: on a phone it
    /// would push the fields below the fold).
    pub form_sidebar: bool,
}

impl Default for MobilePrefs {
    fn default() -> Self {
        Self {
            bottom_nav: true,
            sidebar_drawer: true,
            form_sidebar: false,
        }
    }
}

/// The preferences signal, provided once by the Desk shell.
#[derive(Clone, Copy)]
pub struct MobilePrefsCtx(pub RwSignal<MobilePrefs>);

/// Create the preferences signal, provide it, and (in the browser) keep it in
/// sync with `localStorage`.
///
/// Call once, at the top of the shell: every view that needs a preference reads
/// the same signal, so a change on the settings page is visible to the bar
/// immediately.
pub fn provide_mobile_prefs() -> RwSignal<MobilePrefs> {
    let prefs = RwSignal::new(MobilePrefs::default());
    provide_context(MobilePrefsCtx(prefs));

    #[cfg(target_arch = "wasm32")]
    {
        // Read first, then persist: the load effect has no reactive inputs, so it
        // runs once; the save effect then sees the loaded value and finds storage
        // already equal to it, which is why the first write is skipped.
        Effect::new(move |_| prefs.set(load_prefs()));
        Effect::new(move |_| save_prefs(&prefs.get()));
    }

    prefs
}

/// Read the preferences signal, or a throwaway default when no shell is in
/// scope (standalone view tests, a plugin page rendered outside the Desk).
pub fn use_mobile_prefs() -> RwSignal<MobilePrefs> {
    use_context::<MobilePrefsCtx>()
        .map_or_else(|| RwSignal::new(MobilePrefs::default()), |ctx| ctx.0)
}

/// Whether the bottom navigation bar is on screen: a phone-width viewport, the
/// device preference, and a page that draws the desk chrome at all. `/login` and
/// app-declared bare pages own the whole page, so they get neither the bar nor
/// the room the shell reserves for it.
pub fn bottom_nav_visible(
    is_mobile: Signal<bool>,
    prefs: RwSignal<MobilePrefs>,
    bare_chrome: Memo<bool>,
    pathname: Memo<String>,
    fullscreen: Signal<bool>,
) -> Signal<bool> {
    Signal::derive(move || {
        let is_desk = pathname.get().trim_end_matches('/') == crate::app::DESK_PATH;
        // A full-screen board owns the whole viewport: the bar would cover the
        // bottom of the grid and its padding would shrink it.
        is_mobile.get()
            && prefs.get().bottom_nav
            && !fullscreen.get()
            && (!bare_chrome.get() || is_desk)
    })
}

/// The unread-notification count the chrome shows.
///
/// Shared by the rail's badge, the bottom bar's dot and the tab title, so the
/// three cannot disagree; the "desktop notifications" preference silences all
/// of them at once.
pub fn unread_count(
    notifications: RwSignal<NotificationFeed>,
    settings: RwSignal<NotificationSettings>,
) -> Memo<u64> {
    Memo::new(move |_| {
        if settings.with(|s| s.desktop) {
            notifications.with(|feed| feed.unread)
        } else {
            0
        }
    })
}

#[cfg(target_arch = "wasm32")]
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

/// The stored preferences, or the defaults when there are none or the value is
/// unreadable. A corrupt entry is removed so the next save starts from a clean
/// slate instead of failing forever.
#[cfg(target_arch = "wasm32")]
fn load_prefs() -> MobilePrefs {
    let Some(store) = local_storage() else {
        return MobilePrefs::default();
    };
    let Ok(Some(raw)) = store.get_item(MOBILE_PREFS_STORAGE_KEY) else {
        return MobilePrefs::default();
    };
    match serde_json::from_str::<MobilePrefs>(&raw) {
        Ok(loaded) => loaded,
        Err(_) => {
            let _ = store.remove_item(MOBILE_PREFS_STORAGE_KEY);
            MobilePrefs::default()
        }
    }
}

/// Persist the preferences. Writing only when the stored text differs keeps the
/// effect idempotent: re-running it (or running it right after the load) does not
/// touch storage.
#[cfg(target_arch = "wasm32")]
fn save_prefs(prefs: &MobilePrefs) {
    let Some(store) = local_storage() else {
        return;
    };
    let Ok(raw) = serde_json::to_string(prefs) else {
        return;
    };
    if store
        .get_item(MOBILE_PREFS_STORAGE_KEY)
        .ok()
        .flatten()
        .as_deref()
        == Some(raw.as_str())
    {
        return;
    }
    let _ = store.set_item(MOBILE_PREFS_STORAGE_KEY, &raw);
}

/// The sidebar as a mobile drawer: a fixed overlay that only exists while the
/// user asked for it, so the rail never eats a phone's viewport.
///
/// It is rendered by the shell in place of the persistent rail when `is_mobile`
/// is true. `open` is the drawer's own visibility; the desktop rail's
/// `sidebar_open` is a different signal and is never written from here, so
/// opening the drawer cannot collapse the rail a desktop tab is showing.
#[component]
pub fn MobileSidebarDrawer(
    workspaces: ReadSignal<Vec<Workspace>>,
    /// The workspace the user last picked (see [`crate::app::DeskNav`]).
    picked: ReadSignal<Option<String>>,
    /// Whether the drawer is showing.
    open: RwSignal<bool>,
    /// Pins a workspace when the drawer's header menu chooses one.
    on_pin: Callback<String>,
    /// The command palette's open flag (the drawer closes, then opens it).
    search_open: RwSignal<bool>,
    /// The notifications panel's open flag (the drawer closes, then toggles it).
    notifications_open: RwSignal<bool>,
    /// Unread notifications, drawn as a dot on the bell.
    unread: Memo<u64>,
    /// The current route, for the sidebar's active link.
    current_route: Memo<String>,
) -> impl IntoView {
    let prefs = use_mobile_prefs();
    let i18n = use_i18n();
    // The drawer always renders the rail expanded: there is no room to fold a
    // 16rem rail inside an overlay, and opening the drawer is a request to see
    // the labels.
    let expanded = RwSignal::new(true);

    // The rail's own quick actions close the drawer first, so the user is not
    // left looking at an overlay stacked on an overlay. `Callback` (not a bare
    // closure) because it is `Copy`: the drawer's children are rebuilt on every
    // pass and a moved closure would make the whole subtree `FnOnce`.
    let on_search = Callback::new(move |()| {
        open.set(false);
        search_open.set(true);
    });
    let on_notifications = Callback::new(move |()| {
        open.set(false);
        notifications_open.update(|open| *open = !*open);
    });

    // Turning the drawer off while it is open must close it, not leave a hidden
    // flag set that reappears on the next toggle.
    Effect::new(move |_| {
        if !prefs.get().sidebar_drawer {
            open.set(false);
        }
    });

    // Escape closes the overlay, like every other dialog in the Desk. The
    // listener lives as long as the component and is inert while it is shut.
    let _escape = window_event_listener(leptos::ev::keydown, move |event| {
        if open.get() && event.key() == "Escape" {
            open.set(false);
        }
    });

    // A link inside the sidebar navigates but the drawer is a fixed overlay:
    // without this it stayed open on top of the page the user just asked for.
    Effect::new(move |_| {
        let _ = current_route.get();
        open.set(false);
    });

    // Moving focus into the overlay is part of opening it: a keyboard user
    // should not keep tabbing through the page behind the scrim.
    let close_ref = NodeRef::<leptos::html::Button>::new();
    Effect::new(move |_| {
        if open.get()
            && let Some(button) = close_ref.get()
        {
            let _ = button.focus();
        }
    });

    view! {
        <Show when=move || open.get() && prefs.get().sidebar_drawer>
            // The scrim is the dismissal surface: it covers the page (and the
            // bottom bar) and is not reachable by keyboard, which is why it is
            // hidden from assistive technology instead of being a giant button.
            <div
                class="fixed inset-0 z-[55] bg-overlay md:hidden"
                aria-hidden="true"
                on:click=move |_| open.set(false)
            ></div>
            <div
                // Above the bottom bar (`z-50`): the bar used to paint over the
                // drawer's footer and cover the user menu.
                class="fixed inset-y-0 left-0 z-60 flex md:hidden"
                data-name="MobileSidebarDrawer"
                role="dialog"
                aria-modal="true"
                aria-label=i18n.t("desk.drawer.label")
            >
                <div
                    class="flex h-full w-[var(--sidebar-width)] flex-col bg-sidebar text-sidebar-foreground"
                    // A link to the page already on screen does not change the
                    // route, so the route effect cannot close the drawer: any
                    // navigation target pressed inside closes it.
                    on:click=move |event: leptos::ev::MouseEvent| {
                        let Some(target) = event.target() else { return };
                        let Ok(target) = target.dyn_into::<web_sys::Element>() else {
                            return;
                        };
                        if target.closest("a").ok().flatten().is_some() {
                            open.set(false);
                        }
                    }
                >
                    <div class="flex shrink-0 items-center justify-between gap-2 border-b border-sidebar-border p-3">
                        <span class="text-sm font-semibold">{i18n.t("desk.drawer.title")}</span>
                        <button
                            node_ref=close_ref
                            type="button"
                            aria-label=i18n.t("desk.drawer.close")
                            class="inline-flex size-9 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                            on:click=move |_| open.set(false)
                        >
                            <crate::components::ui::icon::Icon name="X" class="size-4" />
                        </button>
                    </div>
                    <div class="min-h-0 flex-1 overflow-hidden">
                        <Sidebar
                            workspaces=workspaces
                            picked=picked
                            open=expanded.read_only()
                            // `Sidebar`'s toggle is a `WriteSignal`, not a
                            // callback: it flips the expanded state. Any write
                            // here means "fold it", and on mobile that closes
                            // the drawer — writing the visibility signal
                            // toggles `true` to `false` in one step.
                            on_toggle=open.write_only()
                            on_pin=on_pin
                            on_search=move || on_search.run(())
                            on_notifications=move || on_notifications.run(())
                            unread=unread
                            current_route=current_route
                        />
                    </div>
                </div>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::MobilePrefs;

    /// The storage key is a wire contract with values written by previous
    /// builds: defaults must be the documented ones, and an object missing a
    /// later field must still decode (the container has `#[serde(default)]`).
    #[test]
    fn preferences_round_trip_and_old_payloads_default() {
        let defaults = MobilePrefs::default();
        assert!(defaults.bottom_nav);
        assert!(defaults.sidebar_drawer);
        assert!(!defaults.form_sidebar);

        let json = serde_json::to_string(&defaults).unwrap();
        let back: MobilePrefs = serde_json::from_str(&json).unwrap();
        assert_eq!(back, defaults);

        let old: MobilePrefs = serde_json::from_str(r#"{"bottom_nav":false}"#).unwrap();
        assert!(!old.bottom_nav);
        assert!(old.sidebar_drawer);
        assert!(!old.form_sidebar);
    }
}
