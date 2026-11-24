//! Notification panel (P2): the bell in the sidebar unfolds a full-height panel
//! right beside the rail — floating over the page rather than displacing it —
//! with the notification feed, the upcoming events and a short "what's new"
//! list. Settings, mark-all-read and close sit inline to the right of the tabs.
//!
//! The feed is owned by the Desk shell (`NotificationFeed`) so the sidebar badge and
//! the panel never disagree. Everything is fetched when the panel opens; SSR
//! renders it collapsed, so hydration starts from a closed panel and the first
//! open animates the width out of the rail.

use icons::{Bell, Calendar, CheckCheck, Inbox, Settings, Sparkles, X};
use leptos::ev;
use leptos::prelude::*;

use crate::api::drive;
use crate::components::ui::badge::{Badge, BadgeSize, BadgeVariant};
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::dialog::{
    Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle,
};
use crate::components::ui::tabs::{Tabs, TabsContent, TabsList, TabsTrigger};
use crate::desk::session::{
    self as desk_session, DeskEvent, NotificationFeed, NotificationSettings,
};
use crate::i18n::{DeskI18n, use_i18n};
use crate::views::views_components::{Feedback, FeedbackKind};

/// Compact age for the feed rows: `"ahora"`, `"12m"`, `"3h"`, `"4d"`.
fn format_age(rfc3339: &str, i18n: &(impl crate::i18n::Translate + Sync)) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;

    let Some(ts) = chrono::DateTime::parse_from_rfc3339(rfc3339).ok() else {
        return String::new();
    };
    let elapsed = (chrono::Utc::now() - ts.with_timezone(&chrono::Utc))
        .num_seconds()
        .max(0);
    let (step, unit) = match elapsed {
        s if s < MINUTE => return i18n.t("desk.notifications.now"),
        s if s < HOUR => (MINUTE, "m"),
        s if s < DAY => (HOUR, "h"),
        _ => (DAY, "d"),
    };
    format!("{}{unit}", elapsed / step)
}

/// Day and time for an event row.
///
/// Numeric, locale-neutral (`dd/mm/yyyy · HH:MM`): `%a %d %b` rendered English
/// weekday and month names inside an otherwise Spanish panel, and the row has no
/// locale to read from on the client.
fn format_starts(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|ts| {
            ts.with_timezone(&chrono::Utc)
                .format("%d/%m/%Y · %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// Notification panel bound to the feed and preferences owned by the Desk
/// shell, so the sidebar badge stays in step with what the panel shows.
#[component]
pub fn NotificationsPanel(
    open: RwSignal<bool>,
    feed: RwSignal<NotificationFeed>,
    settings: RwSignal<NotificationSettings>,
) -> impl IntoView {
    let i18n = use_i18n();
    let tr = move |key: &str| i18n.with_value(|i18n: &DeskI18n| i18n.t(key));
    let tr_with = move |key: &str, args: &[(&str, &str)]| {
        i18n.with_value(|i18n: &DeskI18n| i18n.t_with(key, args))
    };
    let events = RwSignal::new(Vec::<DeskEvent>::new());
    let settings_open = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let saving = RwSignal::new(false);
    // Why the last save failed. Kept next to the dialog so a failed write can
    // stay open and say so instead of closing as if it had stored the choice.
    let save_error = RwSignal::new(None::<String>);

    // Fetch on every open, so a notification raised elsewhere shows up. The
    // feed and preferences go through the shell's shared refresh; events are
    // panel-only. `loading` stays true until both have been asked for, so an
    // empty feed never flashes the "caught up" state before the first load.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        loading.set(true);
        crate::app::refresh_notifications(feed, settings);
        drive(async move {
            if let Ok(fresh_events) = desk_session::events().await {
                events.set(fresh_events.items);
            }
            loading.set(false);
        });
    });

    // Closing the panel has to take the settings dialog with it: the dialog is
    // mounted outside the panel's open gate, so leaving it set would strand it
    // over a closed panel.
    let close = move || {
        settings_open.set(false);
        open.set(false);
    };

    // Escape closes the panel. The listener lives as long as the component;
    // the guard keeps it inert while the panel is shut.
    let _escape = window_event_listener(ev::keydown, move |event| {
        if open.get() && event.key() == "Escape" {
            if settings_open.get() {
                settings_open.set(false);
            } else {
                open.set(false);
            }
        }
    });

    let save_settings = move || {
        let settings = settings;
        let feed = feed;
        saving.set(true);
        save_error.set(None);
        drive(async move {
            // Snapshot before awaiting: the async block runs outside the
            // reactive context, so a tracked read here only warns.
            let next = settings.get_untracked();
            match desk_session::put_notification_settings(&next).await {
                Ok(()) => {
                    // Re-read both from the server: the saved preferences decide
                    // whether the sidebar draws its badge, so the shell has to
                    // see them too.
                    crate::app::refresh_notifications(feed, settings);
                    saving.set(false);
                    settings_open.set(false);
                }
                Err(message) => {
                    // The write did not land: keep the dialog open with the
                    // user's toggles so they can retry, and say why.
                    save_error.set(Some(tr_with(
                        "desk.notifications.save_error",
                        &[("message", &message)],
                    )));
                    saving.set(false);
                }
            }
        });
    };

    view! {
        // An overlay anchored to the rail, not a column in the shell's flex
        // row: the panel unfolds from the sidebar's right edge — right next to
        // the bell that opened it — but floats *over* the page, so opening it
        // never reflows the content behind. The `aside` is a zero-width anchor
        // pinned to the rail's edge, so the panel tracks the rail as it
        // animates its own width; the inner panel carries the width transition
        // and the fade. It starts below the top bar (`top-12`), which is sticky
        // and would otherwise paint over the panel's tabs. Everything stays
        // mounted so the transition actually runs; closed it is a `w-0` clip
        // whose contents are faded out and inert.
        <aside class="relative h-full w-0 shrink-0" aria-hidden=move || (!open.get()).to_string()>
            <div
                class=move || {
                    if open.get() {
                        "absolute left-0 top-12 bottom-0 z-30 overflow-hidden transition-all duration-300 ease-out max-md:fixed max-md:inset-x-0 max-md:w-full w-96 max-md:w-full"
                    } else {
                        "absolute left-0 top-12 bottom-0 z-30 overflow-hidden transition-all duration-300 ease-out max-md:fixed max-md:inset-x-0 max-md:w-full w-0"
                    }
                }
            >
                <div
                    class=move || format!(
                        "flex h-full w-96 flex-col overflow-hidden border-r border-border/60 bg-background shadow-2xl transition-opacity duration-200 max-md:w-full {}",
                        if open.get() { "opacity-100" } else { "pointer-events-none opacity-0" }
                    )
                    inert=move || !open.get()
                >
                    <Tabs id="notification-tabs" default_value="notifications" class="flex min-h-0 flex-1 flex-col gap-0">
                        {/* Tabs on the left, panel actions inline on the right */}
                        <div class="flex shrink-0 items-center gap-2 border-b border-border/60 px-3 py-2">
                            <TabsList class="h-7">
                                <TabsTrigger value="notifications" class="h-6 text-xs">
                                    {tr("desk.notifications.title")}
                                </TabsTrigger>
                                <TabsTrigger value="events" class="h-6 text-xs">
                                    {tr("desk.notifications.tab.events")}
                                </TabsTrigger>
                                <TabsTrigger value="whats-new" class="h-6 text-xs">
                                    {tr("desk.notifications.tab.whats_new")}
                                </TabsTrigger>
                            </TabsList>

                            <div class="ml-auto flex items-center gap-1">
                                <Button
                                    variant=ButtonVariant::Ghost
                                    size=ButtonSize::IconSm
                                    class="size-7"
                                    attr:title=tr("desk.notifications.settings")
                                    attr:aria-label=tr("desk.notifications.settings")
                                    on:click=move |_| {
                                        save_error.set(None);
                                        settings_open.set(true);
                                    }
                                >
                                    <Settings class="size-4" />
                                </Button>
                                <Button
                                    variant=ButtonVariant::Ghost
                                    size=ButtonSize::IconSm
                                    class="size-7"
                                    attr:title=tr("desk.notifications.mark_all_read")
                                    attr:aria-label=tr("desk.notifications.mark_all_read")
                                    on:click=move |_| crate::desk::notifications::mark_all_read(feed)
                                >
                                    <CheckCheck class="size-4" />
                                </Button>
                                <Button
                                    variant=ButtonVariant::Ghost
                                    size=ButtonSize::IconSm
                                    class="size-7"
                                    attr:title=tr("common.close")
                                    attr:aria-label=tr("common.close")
                                    on:click=move |_| close()
                                >
                                    <X class="size-4" />
                                </Button>
                            </div>
                        </div>

                        <TabsContent value="notifications" class="mt-0 min-h-0 flex-1 overflow-y-auto p-3 max-md:pb-16">
                            <NotificationFeedList feed=feed loading=loading.read_only() />
                        </TabsContent>

                        <TabsContent value="events" class="mt-0 min-h-0 flex-1 overflow-y-auto p-3 max-md:pb-16">
                            <Events events=events.read_only() />
                        </TabsContent>

                        <TabsContent value="whats-new" class="mt-0 min-h-0 flex-1 overflow-y-auto p-3 max-md:pb-16">
                            <WhatsNew />
                        </TabsContent>
                    </Tabs>
                </div>
            </div>
        </aside>

        <Dialog>
            <DialogContent open=settings_open show_close_button=true class="max-w-sm">
                <DialogHeader>
                    <DialogTitle>{tr("desk.notifications.settings")}</DialogTitle>
                    <DialogDescription>
                        {tr("desk.notifications.settings_description")}
                    </DialogDescription>
                </DialogHeader>

                <div class="flex flex-col gap-1">
                    <SettingsToggle
                        label="desk.notifications.setting.badge_label"
                        help="desk.notifications.setting.badge_help"
                        settings=settings
                        key="desktop"
                    />
                    <SettingsToggle
                        label="desk.notifications.setting.welcome_label"
                        help="desk.notifications.setting.welcome_help"
                        settings=settings
                        key="notify_on_login"
                    />
                </div>

                <Feedback
                    kind=FeedbackKind::Error
                    message=Signal::derive(move || save_error.get())
                />

                <div class="flex justify-end gap-2">
                    <Button
                        variant=ButtonVariant::Ghost
                        on:click=move |_| settings_open.set(false)
                    >
                        {tr("common.cancel")}
                    </Button>
                    <Button attr:disabled=move || saving.get() on:click=move |_| save_settings()>
                        {tr("common.save")}
                    </Button>
                </div>
            </DialogContent>
        </Dialog>
    }
}

/// The notification rows, shared by the panel's notifications tab and the Desk
/// home's bell dropdown.
///
/// The dropdown is not a second notification design: it shows this list, so a
/// change to a row (the bell glyph, the unread dot, the age format, the dismiss
/// affordance) reaches both entry points at once.
#[component]
pub(crate) fn NotificationFeedList(
    feed: RwSignal<NotificationFeed>,
    loading: ReadSignal<bool>,
) -> impl IntoView {
    // Taken once: `use_navigate` reads router context, so calling it per click
    // inside a `view!` closure is both wasteful and context-sensitive.
    let navigate = leptos_router::hooks::use_navigate();
    let i18n = use_i18n();

    view! {
        {move || {
            let items = feed.get().items;
            if items.is_empty() && !loading.get() {
                return view! {
                    <div class="flex flex-col items-center gap-2 px-4 py-12 text-center">
                        <Inbox class="size-8 text-muted-foreground/60" />
                        <p class="text-sm font-medium">{i18n.t("desk.notifications.empty_title")}</p>
                        <p class="text-xs text-muted-foreground">
                            {i18n.t("desk.notifications.empty_body")}
                        </p>
                    </div>
                }.into_any();
            }

            view! {
                <ul class="flex flex-col divide-y divide-border/60">
                    {items.into_iter().map(|item| {
                        let title = item.title.clone();
                        let description = item.description.clone();
                        let route = item.route.clone();
                        let id = item.id.clone();
                        let dismiss_id = item.id.clone();
                        let nav_route = item.route.clone();
                        let nav = navigate.clone();
                        let age = format_age(&item.created_at, &i18n);
                        let unread = !item.is_read;

                        view! {
                            <li class="flex items-start gap-1">
                                <button
                                    type="button"
                                    class="flex w-full items-start gap-3 px-1 py-2.5 text-left transition-colors hover:bg-muted/60"
                                    data-dropdown-close="true"
                                    on:click=move |_| {
                                        crate::desk::notifications::mark_read(&id, feed);
                                        if !route.trim().is_empty() {
                                            nav(&nav_route, Default::default());
                                        }
                                    }
                                >
                                    <span class="relative mt-0.5 shrink-0">
                                        <Bell class="size-4 text-muted-foreground" />
                                        {unread.then(|| view! {
                                            <span class="absolute -right-1 -top-1 size-2 rounded-full bg-primary"></span>
                                        })}
                                    </span>
                                    <span class="flex min-w-0 flex-1 flex-col gap-0.5">
                                        <span class="truncate text-sm font-medium">{title}</span>
                                        {(!description.trim().is_empty()).then(|| view! {
                                            <span class="line-clamp-2 text-xs text-muted-foreground">{description}</span>
                                        })}
                                    </span>
                                    <span class="shrink-0 text-[10px] text-muted-foreground">{age}</span>
                                </button>
                                <button
                                    type="button"
                                    class="mt-1 inline-flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 max-md:size-10"
                                    title=i18n.t("desk.notifications.dismiss")
                                    aria-label=i18n.t("desk.notifications.dismiss_label")
                                    data-dropdown-close="true"
                                    on:click=move |_| crate::desk::notifications::dismiss(&dismiss_id, feed)
                                >
                                    <X class="size-3.5" />
                                </button>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            }.into_any()
        }}
    }
}

/// The events tab.
#[component]
fn Events(events: ReadSignal<Vec<DeskEvent>>) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        {move || {
            let list = events.get();
            if list.is_empty() {
                return view! {
                    <div class="flex flex-col items-center gap-2 px-4 py-12 text-center">
                        <Calendar class="size-8 text-muted-foreground/60" />
                        <p class="text-sm font-medium">{i18n.t("desk.notifications.events_empty_title")}</p>
                        <p class="text-xs text-muted-foreground">
                            {i18n.t("desk.notifications.events_empty_body")}
                        </p>
                    </div>
                }.into_any();
            }

            view! {
                <ul class="flex flex-col divide-y divide-border/60">
                    {list.into_iter().map(|event| {
                        let title = event.title.clone();
                        let location = event.location.clone();
                        let starts = format_starts(&event.starts_on);
                        let description = event.description.clone();

                        view! {
                            <li class="flex items-start gap-3 px-1 py-2.5">
                                <Calendar class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
                                <div class="flex min-w-0 flex-1 flex-col gap-0.5">
                                    <span class="truncate text-sm font-medium">{title}</span>
                                    <span class="text-xs text-muted-foreground">{starts}</span>
                                    {(!location.trim().is_empty()).then(|| view! {
                                        <span class="truncate text-xs text-muted-foreground">{location}</span>
                                    })}
                                    {(!description.trim().is_empty()).then(|| view! {
                                        <span class="line-clamp-2 text-xs text-muted-foreground">{description}</span>
                                    })}
                                </div>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            }.into_any()
        }}
    }
}

/// The what's-new tab: what changed in this build, no backend needed.
#[component]
fn WhatsNew() -> impl IntoView {
    // Catalogue keys, not text: the same list renders in the negotiated locale.
    const NOTES: &[(&str, &str)] = &[
        (
            "desk.notifications.whats_new.panel_title",
            "desk.notifications.whats_new.panel_body",
        ),
        (
            "desk.notifications.whats_new.doctypes_title",
            "desk.notifications.whats_new.doctypes_body",
        ),
        (
            "desk.notifications.whats_new.settings_title",
            "desk.notifications.whats_new.settings_body",
        ),
    ];

    let i18n = use_i18n();
    view! {
        <ul class="flex flex-col gap-3">
            {NOTES.iter().map(|(title, body)| {
                let title = i18n.t(title);
                let body = i18n.t(body);
                view! {
                    <li class="flex items-start gap-3 rounded-lg border border-border/60 p-3">
                        <Sparkles class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
                        <div class="flex min-w-0 flex-1 flex-col gap-0.5">
                            <span class="text-sm font-medium">{title}</span>
                            <span class="text-xs text-muted-foreground">{body}</span>
                        </div>
                        <Badge variant=BadgeVariant::Secondary size=BadgeSize::Sm>
                            {i18n.t("desk.notifications.whats_new.badge")}
                        </Badge>
                    </li>
                }
            }).collect_view()}
        </ul>
    }
}

/// One row of the settings dialog: a label, its help text and the toggle.
#[component]
fn SettingsToggle(
    label: &'static str,
    help: &'static str,
    settings: RwSignal<NotificationSettings>,
    key: &'static str,
) -> impl IntoView {
    let i18n = use_i18n();
    let label = i18n.t(label);
    let aria_label = label.clone();
    let help = i18n.t(help);
    view! {
        <div class="flex items-center justify-between gap-4 rounded-lg border border-border/60 p-3">
            <div class="flex min-w-0 flex-col gap-0.5">
                <span class="text-sm font-medium">{label}</span>
                <span class="text-xs text-muted-foreground">{help}</span>
            </div>
            <button
                type="button"
                role="switch"
                aria-label=aria_label
                class=move || {
                    let on = match key {
                        "desktop" => settings.get().desktop,
                        _ => settings.get().notify_on_login,
                    };
                    if on {
                        "inline-flex h-6 w-11 shrink-0 items-center rounded-full bg-primary transition-colors focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                    } else {
                        "inline-flex h-6 w-11 shrink-0 items-center rounded-full bg-input transition-colors focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                    }
                }
                aria-checked=move || match key {
                    "desktop" => settings.get().desktop,
                    _ => settings.get().notify_on_login,
                }.to_string()
                on:click=move |_| {
                    settings.update(|s| match key {
                        "desktop" => s.desktop = !s.desktop,
                        _ => s.notify_on_login = !s.notify_on_login,
                    });
                }
            >
                <span class=move || {
                    let on = match key {
                        "desktop" => settings.get().desktop,
                        _ => settings.get().notify_on_login,
                    };
                    if on {
                        "block size-5 rounded-full bg-background shadow-lg translate-x-5 transition-transform pointer-events-none"
                    } else {
                        "block size-5 rounded-full bg-background shadow-lg translate-x-0 transition-transform pointer-events-none"
                    }
                }></span>
            </button>
        </div>
    }
}
