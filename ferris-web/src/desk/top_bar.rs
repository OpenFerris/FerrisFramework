//! The Desk home's top bar.
//!
//! `/desk` is the one page rendered without the shell chrome (no sidebar, no
//! breadcrumb top bar): the grid *is* the page. That left the desk home without
//! the controls every other view has — search, notifications, the user menu —
//! so this bar brings the same chrome to it, reusing the same pieces rather
//! than growing a second design:
//!
//! - the `ferris-topbar` shell and `h-12` height the shared `TopBar` uses;
//! - the global [`DropdownMenu`] for both popups;
//! - [`NotificationFeedList`], the same rows the sidebar's notification panel
//!   draws, so a change to a row reaches both;
//! - [`ThemeDialog`] from the sidebar's module menu;
//! - [`logout_callback`] from the sidebar's user card.
//!
//! Search is a button, not an input: the query is typed in the global search
//! dialog, and a real input here would steal focus and duplicate its state.
//!
//! The bar is rendered by [`DeskApp`](crate::app::DeskApp) only on the `/desk`
//! route. It is part of the shell because the state it drives (the search dialog,
//! the notification feed, the signed-in user) is owned there; the grid does not
//! have to know about any of it.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_navigate;

use crate::WebUser;
use crate::app::{DeskNav, Workspace, WorkspaceItemKind};
use crate::components::desk::top_bar::TopBarMobileMenu;
use crate::components::ui::avatar::{Avatar, AvatarFallback};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dropdown_menu::{
    DropdownMenu, DropdownMenuAction, DropdownMenuActionVariant, DropdownMenuAlign,
    DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
};
use crate::components::ui::icon::Icon;
use crate::desk::notifications_panel::NotificationFeedList;
use crate::desk::session::{self as desk_session, NotificationFeed, NotificationSettings};
use crate::desk::theme_dialog::ThemeDialog;
use crate::desk::user_menu::logout_callback;
use icons::{Bell, Search};

/// The built-in Users module, as registered by `ferris-auth`.
///
/// Used only as the fallback for "Mi perfil" until `/api/desk-layout` names
/// the real route: the feed resolves every doctype to its module, and this
/// crate deliberately does not depend on `ferris-auth` to import the constant.
/// Once the feed lands, [`doctype_route`] finds the route the sidebar itself
/// uses, so a renamed or re-moduled Users workspace still opens correctly.
const USERS_MODULE: &str = "Users";

/// The top bar shown on `/desk`.
///
/// All state is passed in from [`DeskApp`](crate::app::DeskApp) rather than read
/// from a second set of contexts: the search dialog, the notification feed and
/// the user identity have exactly one owner, and this bar is a view of them.
#[component]
pub fn DeskTopBar(
    user: RwSignal<WebUser>,
    search_open: RwSignal<bool>,
    notifications: RwSignal<NotificationFeed>,
    settings: RwSignal<NotificationSettings>,
) -> impl IntoView {
    let nav = expect_context::<DeskNav>();
    let navigate = use_navigate();
    let mobile_menu = use_context::<TopBarMobileMenu>();

    let theme_open = RwSignal::new(false);
    let on_themes = Callback::new(move |_| theme_open.set(true));
    let on_edit_profile = Callback::new(move |_: ()| {
        // Read untracked: the menu item is an action, not a reactive view. The
        // feed is normally loaded by the time the user opens this menu.
        let target = {
            let user = user.get_untracked();
            let workspaces = nav.workspaces.get_untracked();
            user_form_route(&workspaces, &user)
        };
        navigate(&target, Default::default());
    });
    let on_logout = logout_callback(user);

    let full_name = move || user.with(|u| u.full_name.clone());
    let email = move || user.with(|u| u.email.clone());
    let initial = move || {
        user.with(|u| {
            u.full_name
                .trim()
                .chars()
                .next()
                .or_else(|| u.email.trim().chars().next())
                .unwrap_or('U')
        })
    };

    // The bell's dot honours the same preference as the sidebar badge and the
    // tab title: turning the badge off turns it off everywhere.
    let unread = Memo::new(move |_| {
        if settings.with(|s| s.desktop) {
            notifications.with(|f| f.unread)
        } else {
            0
        }
    });
    let notification_label = move || {
        let count = unread.get();
        if count > 0 {
            format!("Avisos ({count} sin leer)")
        } else {
            "Avisos".to_string()
        }
    };

    // The feed list is the same component the panel uses; the dropdown has no
    // loading phase of its own (the shell fetches on sign-in and opening the
    // menu refreshes in the background), so it always renders the feed it has.
    let loading = RwSignal::new(false).read_only();

    view! {
        <header class="ferris-topbar sticky top-0 z-30 border-b">
            <div class="grid h-12 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-3 px-3">
                <div class="flex items-center gap-1">
                    {mobile_menu.map(|menu| view! {
                        <Show when=move || menu.available.get()>
                            <button
                                type="button"
                                aria-label="Abrir menú"
                                class="inline-flex size-9 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 md:hidden"
                                on:click=move |_| menu.open.update(|open| *open = !*open)
                            >
                                <Icon name="Menu" class="size-5" />
                            </button>
                        </Show>
                    })}
                    {/* The system mark: the same primary square the login gate
                        uses, and a link home from every corner of the desk. */}
                    <A
                        href=crate::app::DESK_PATH
                        attr:class="flex size-9 items-center justify-center rounded-lg bg-primary text-sm font-bold text-primary-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                        attr:title="Ferris Desk"
                        attr:aria-label="Inicio del escritorio"
                    >
                        "F"
                    </A>
                </div>

                <div class="flex min-w-0 justify-center px-1">
                    <button
                        type="button"
                        class="flex h-9 max-md:h-10 w-full max-w-md items-center gap-2 rounded-md border border-input bg-background px-3 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                        aria-label="Buscar en el escritorio"
                        on:click=move |_| search_open.set(true)
                    >
                        <Search class="size-4 shrink-0" />
                        <span class="min-w-0 flex-1 truncate text-left">"Buscar…"</span>
                        <kbd class="hidden shrink-0 rounded border border-border px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground sm:inline">
                            "Ctrl K"
                        </kbd>
                    </button>
                </div>

                <div class="flex items-center gap-1">
                    <DropdownMenu align=DropdownMenuAlign::End>
                        <DropdownMenuTrigger as_child=true>
                            <Button
                                variant=ButtonVariant::Ghost
                                size=ButtonSize::Icon
                                class="relative size-9"
                                attr:title=notification_label
                                attr:aria-label=notification_label
                                on:click=move |_| {
                                    // Opening the menu is the natural moment to
                                    // catch up: the stream is live, but a page
                                    // load may have missed old unread rows.
                                    crate::app::refresh_notifications(notifications, settings);
                                }
                            >
                                <Bell class="size-5" />
                                {move || (unread.get() > 0).then(|| view! {
                                    <span class="absolute right-2 top-2 size-2 rounded-full bg-primary"></span>
                                })}
                            </Button>
                        </DropdownMenuTrigger>

                        <DropdownMenuContent class="w-[24rem] max-w-[calc(100vw-1rem)] p-0">
                            <div class="flex items-center justify-between gap-2 border-b border-border px-3 py-2">
                                <span class="text-sm font-semibold">"Avisos"</span>
                                <Button
                                    variant=ButtonVariant::Ghost
                                    size=ButtonSize::Sm
                                    class="h-7 gap-1.5 px-2 text-xs"
                                    on:click=move |_| {
                                        crate::desk::notifications::mark_all_read(notifications);
                                    }
                                >
                                    <Icon name="check-check" class="size-3.5" />
                                    "Marcar todo como leído"
                                </Button>
                            </div>
                            <div class="max-h-96 overflow-y-auto p-1">
                                <NotificationFeedList feed=notifications loading=loading />
                            </div>
                        </DropdownMenuContent>
                    </DropdownMenu>

                    <DropdownMenu align=DropdownMenuAlign::End>
                        <DropdownMenuTrigger as_child=true>
                            <button
                                type="button"
                                class="flex size-9 items-center justify-center rounded-full transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 max-md:size-10"
                                title=full_name
                                aria-label="Abrir menú de usuario"
                            >
                                <Avatar>
                                    <AvatarFallback class="bg-primary/10 text-primary">
                                        {initial}
                                    </AvatarFallback>
                                </Avatar>
                            </button>
                        </DropdownMenuTrigger>

                        <DropdownMenuContent class="w-64">
                            <div class="px-2 py-1.5">
                                <p class="truncate text-sm font-medium">{full_name}</p>
                                <p class="truncate text-xs text-muted-foreground">{email}</p>
                            </div>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem>
                                <DropdownMenuAction on_click=on_edit_profile>
                                    <Icon name="user-round" class="size-4" />
                                    "Mi perfil"
                                </DropdownMenuAction>
                            </DropdownMenuItem>
                            <DropdownMenuItem>
                                <DropdownMenuAction on_click=on_themes>
                                    <Icon name="palette" class="size-4" />
                                    "Cambiar tema"
                                </DropdownMenuAction>
                            </DropdownMenuItem>
                            <DropdownMenuSeparator />
                            <DropdownMenuItem>
                                <DropdownMenuAction
                                    variant=DropdownMenuActionVariant::Destructive
                                    on_click=on_logout
                                >
                                    <Icon name="log-out" class="size-4" />
                                    "Cerrar sesión"
                                </DropdownMenuAction>
                            </DropdownMenuItem>
                        </DropdownMenuContent>
                    </DropdownMenu>
                </div>
            </div>
        </header>

        <ThemeDialog open=theme_open />
    }
}

/// Where the "Mi perfil" menu item goes: the form route of the signed-in user's
/// own User document.
///
/// The route is read from the workspaces feed (`doctype == "User"`) so it follows
/// whatever module the app assigned, instead of hardcoding one. The built-in
/// Users module is only the fallback while the feed is still loading.
///
/// `pub(crate)` because the sidebar's [`UserMenu`](crate::desk::UserMenu) offers
/// the same destination: two copies of this derivation would drift.
pub(crate) fn user_form_route(workspaces: &[Workspace], user: &WebUser) -> String {
    let base =
        doctype_route(workspaces, "User").unwrap_or_else(|| format!("/desk/{USERS_MODULE}/User"));
    let document = if user.id.trim().is_empty() {
        user.email.trim()
    } else {
        user.id.trim()
    };
    if document.is_empty() {
        base
    } else {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            desk_session::path_segment(document)
        )
    }
}

/// Find the list route of a doctype by walking every workspace, groups
/// included.
fn doctype_route(workspaces: &[Workspace], doctype: &str) -> Option<String> {
    fn walk(items: &[crate::app::WorkspaceItem], doctype: &str) -> Option<String> {
        for item in items {
            if item.kind == WorkspaceItemKind::Doctype
                && (item.doctype.as_deref() == Some(doctype) || item.name == doctype)
            {
                return item.route.clone();
            }
            if let Some(route) = walk(&item.children, doctype) {
                return Some(route);
            }
        }
        None
    }

    workspaces
        .iter()
        .find_map(|workspace| walk(&workspace.items, doctype))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::WorkspaceItem;

    fn user(id: &str) -> WebUser {
        WebUser {
            id: id.into(),
            email: "ada@example.com".into(),
            full_name: "Ada Lovelace".into(),
            username: Some("ada".into()),
            roles: vec!["System Manager".into()],
            user_type: "System User".into(),
        }
    }

    fn workspace(items: Vec<WorkspaceItem>) -> Workspace {
        Workspace {
            name: "users".into(),
            label: "Users".into(),
            module: USERS_MODULE.into(),
            icon: None,
            items,
        }
    }

    fn doctype_item(route: &str) -> WorkspaceItem {
        WorkspaceItem {
            name: "User".into(),
            label: "User".into(),
            kind: WorkspaceItemKind::Doctype,
            doctype: Some("User".into()),
            icon: None,
            route: Some(route.into()),
            href: None,
            open_in_new_tab: false,
            children: Vec::new(),
        }
    }

    #[test]
    fn the_feed_route_wins_and_the_document_name_is_encoded() {
        let workspaces = vec![workspace(vec![doctype_item("/desk/People/User")])];
        assert_eq!(
            user_form_route(&workspaces, &user("ada@example.com")),
            "/desk/People/User/ada%40example.com"
        );
    }

    #[test]
    fn the_builtin_module_is_the_fallback_before_the_feed_lands() {
        assert_eq!(
            user_form_route(&[], &user("Administrator")),
            "/desk/Users/User/Administrator"
        );
    }
}
