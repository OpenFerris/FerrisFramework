//! User menu component

use crate::WebUser;
use crate::api::drive;
use crate::app::DeskNav;
use crate::components::ui::avatar::{Avatar, AvatarFallback};
use crate::components::ui::separator::Separator;
use crate::desk::LanguagePicker;
use crate::desk::session as desk_session;
use crate::i18n::{DeskI18n, use_i18n};
use icons::ChevronDown;
use leptos::prelude::*;
use leptos_router::components::A;

/// The sign-out action, shared by the sidebar's user card and the Desk home's
/// avatar menu.
///
/// Both menus offer the same three effects (end the session server-side, make
/// the desk render as a guest, land on the login page rather than on a home the
/// server would bounce back to), so they share one callback instead of two
/// copies that could drift. It must be called from inside a component owner:
/// `use_navigate` reads router context, which is not available once the future
/// runs.
pub(crate) fn logout_callback(user: RwSignal<WebUser>) -> Callback<()> {
    let navigate = leptos_router::hooks::use_navigate();
    // A `Callback` (rather than a bare closure) so the menu markup can call it
    // from a nested `move` closure without consuming it.
    Callback::new(move |_: ()| {
        let navigate = navigate.clone();
        drive(async move {
            let _ = desk_session::logout().await;
            user.set(WebUser::guest());
            // Land on the standalone login page rather than the desk home,
            // which the server would only bounce back here.
            navigate(crate::app::LOGIN_PATH, Default::default());
        });
    })
}

#[component]
pub fn UserMenu(user: RwSignal<WebUser>, expanded: ReadSignal<bool>) -> impl IntoView {
    let (open, set_open) = signal(false);
    let i18n = use_i18n();
    let tr = move |key: &str| i18n.with_value(|i18n: &DeskI18n| i18n.t(key));
    let nav = expect_context::<DeskNav>();
    let full_name = move || user.with(|u| u.full_name.clone());
    let email = move || user.with(|u| u.email.clone());
    let initial = move || user.with(|u| u.full_name.chars().next().unwrap_or('U'));

    let on_logout = logout_callback(user);

    // Where "Mi perfil" goes: the signed-in user's own User form, resolved from
    // the workspaces feed exactly as the Desk home's menu does. `/settings/profile`
    // and `/settings/account` do not exist — the settings page only knows
    // general/permissions/about — so those links landed on a blank section.
    // Read untracked: the destination is computed once for the link, and the feed
    // has normally landed by the time the sidebar draws.
    let profile_href = crate::desk::top_bar::user_form_route(
        &nav.workspaces.get_untracked(),
        &user.get_untracked(),
    );

    // Where the menu opens. Expanded, it drops up from the full-width row.
    // Collapsed to the rail it becomes a flyout to the right of the avatar,
    // vertically centred: opening upward from a 2.5rem-wide rail would cover
    // the nav and read as detached from the button it belongs to.
    let menu_anchor = move || {
        if expanded.get() {
            "absolute bottom-full left-0 mb-2 z-20"
        } else {
            "absolute left-full top-1/2 z-20 ml-2 -translate-y-1/2"
        }
    };

    view! {
        <div class="relative">
            <button
                class=move || if expanded.get() {
                    "w-full flex items-center gap-3 p-2 rounded-lg hover:bg-accent transition-colors focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                } else {
                    "w-10 h-10 mx-auto flex items-center justify-center rounded-lg hover:bg-accent transition-colors focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                }
                on:click=move |_| set_open.update(|o| *o = !*o)
                title=move || user.with(|u| u.full_name.clone())
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
            >
                <Avatar>
                    <AvatarFallback class="bg-primary/10 text-primary">
                        {initial}
                    </AvatarFallback>
                </Avatar>
                // Collapsed to the rail, the card is the avatar and nothing
                // else. These are rendered conditionally rather than hidden with
                // a `hidden` class: the chevron span also carries `inline-flex`,
                // and `.inline-flex` wins over `.hidden` in the compiled sheet
                // (same specificity, later rule), which left the chevron on
                // screen next to the avatar.
                {move || expanded.get().then(|| view! {
                    <div class="flex-1 text-left min-w-0">
                        <p class="text-sm font-medium text-foreground truncate">{full_name}</p>
                        <p class="text-xs text-muted-foreground truncate">{email}</p>
                    </div>
                })}
                {move || expanded.get().then(|| view! {
                    <span
                        class="inline-flex text-muted-foreground transition-transform duration-200"
                        class:rotate-180=open
                    >
                        <ChevronDown class="size-4" />
                    </span>
                })}
            </button>

            {move || {
                if open.get() {
                    view! {
                        <div class=menu_anchor>
                            <div class="w-64 bg-popover text-popover-foreground rounded-lg shadow-lg border border-border py-1">
                                <div class="px-4 py-2 border-b border-border">
                                    <p class="text-sm font-medium text-foreground">{full_name}</p>
                                    <p class="text-xs text-muted-foreground">{email}</p>
                                </div>
                                <A href=profile_href.clone() attr:class="block px-4 py-2 text-sm text-popover-foreground hover:bg-accent">{tr("desk.user_menu.profile")}</A>
                                <A href="/settings" attr:class="block px-4 py-2 text-sm text-popover-foreground hover:bg-accent">{tr("nav.settings")}</A>
                                <LanguagePicker />
                                <Separator class="my-1" />
                                <button
                                    class="w-full text-left block px-4 py-2 text-sm text-destructive hover:bg-accent"
                                    on:click=move |_| on_logout.run(())
                                >
                                    {tr("nav.logout")}
                                </button>
                            </div>
                        </div>
                    }.into_any()
                } else {
                    view! { <span></span> }.into_any()
                }
            }}
        </div>
    }
}
