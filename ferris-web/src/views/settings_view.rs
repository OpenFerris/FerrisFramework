//! Settings: site presentation, diagnostics, custom permissions and version.
//!
//! The page used to be a single card that probed a hardcoded `Sales Invoice`
//! list. It now reads the real site settings the shell already loaded, offers a
//! generic diagnostic (the desk layout is a real engine-backed read, not a
//! placeholder), and gives the Role Permissions Manager its own section.
//!
//! Sections are addressed by the `/settings/*rest` route, so every tab is a
//! real URL that can be bookmarked and survives a reload.

use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::use_params_map;

use crate::app::use_mobile_prefs;
use crate::app::use_site_state;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::card::{Card, CardContent, CardDescription, CardHeader, CardTitle};
use crate::components::ui::checkbox::Checkbox;
use crate::components::ui::table as t;
use crate::desk::session;
use crate::i18n::use_i18n;
use crate::views::settings_docperm::DocPermMatrix;
use crate::views::settings_doctype_builder::DocTypeBuilder;
use crate::views::settings_permissions::CustomPermissionManager;
use crate::views::settings_security::SecuritySection;
use crate::views::views_components::{Feedback, FeedbackKind, LoadingLines, PageHeader};

#[component]
pub fn SettingsView() -> impl IntoView {
    let params = use_params_map();
    let i18n = use_i18n();
    let tr = move |key: &str| {
        i18n.try_with_value(|i| i.t(key))
            .unwrap_or_else(|| key.to_string())
    };
    // A new tab may introduce a key the Core catalogue does not carry yet; the
    // fallback keeps it readable instead of printing the key.
    let tr_or = move |key: &str, fallback: &str| {
        i18n.try_with_value(|i| {
            let value = i.t(key);
            if value == key {
                fallback.to_string()
            } else {
                value
            }
        })
        .unwrap_or_else(|| fallback.to_string())
    };
    let section = Signal::derive(move || {
        params.with(|params| {
            params
                .get("rest")
                .or_else(|| params.get("section"))
                .unwrap_or_default()
        })
    });
    let is = move |name: &'static str| {
        let section = section;
        Signal::derive(move || {
            let current = section.get();
            let current = current.trim_end_matches('/');
            match name {
                // An unknown section (`/settings/profile`, an old bookmark)
                // lands on General instead of a page with no section at all.
                "general" => {
                    current.is_empty()
                        || current == "general"
                        || !matches!(current, "permissions" | "doctypes" | "about" | "security")
                }
                other => current == other,
            }
        })
    };

    let general = is("general");
    let permissions = is("permissions");
    let doctypes = is("doctypes");
    let security = is("security");
    let about = is("about");
    view! {
        <div class="mx-auto flex h-full w-full max-w-6xl flex-col gap-4 overflow-y-auto p-4">
            <PageHeader
                title=tr("nav.settings")
                description=tr("views.settings.description")
            />

            <nav
                class="flex flex-wrap items-center gap-1 rounded-lg border border-border bg-surface p-1"
                aria-label=tr("views.settings.sections_aria")
            >
                <SettingsTab href="/settings" label=tr("views.settings.tab_general") active=general />
                <SettingsTab
                    href="/settings/permissions"
                    label=tr("views.settings.tab_permissions")
                    active=permissions
                />
                <SettingsTab
                    href="/settings/doctypes"
                    label=tr_or("views.settings.tab_doctypes", "DocTypes")
                    active=doctypes
                />
                <SettingsTab href="/settings/security" label=tr("views.settings.tab_security") active=security />
                <SettingsTab href="/settings/about" label=tr("views.settings.tab_about") active=about />
            </nav>

            <Show when=move || general.get()>
                <GeneralSection />
            </Show>
            <Show when=move || permissions.get()>
                <div class="flex flex-col gap-4">
                    <DocPermMatrix />
                    <CustomPermissionManager />
                </div>
            </Show>
            <Show when=move || doctypes.get()>
                <DocTypeBuilder />
            </Show>
            <Show when=move || security.get()>
                <SecuritySection />
            </Show>
            <Show when=move || about.get()>
                <AboutSection />
            </Show>
        </div>
    }
}

/// One tab of the settings sub-navigation.
#[component]
fn SettingsTab(
    #[prop(into)] href: String,
    #[prop(into)] label: String,
    #[prop(into)] active: Signal<bool>,
) -> impl IntoView {
    let class = move || {
        if active.get() {
            "inline-flex h-8 items-center rounded-md bg-background px-3 text-sm font-medium text-foreground shadow-xs max-md:h-10 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
        } else {
            "inline-flex h-8 items-center rounded-md px-3 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground max-md:h-10 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
        }
    };
    view! {
        <A href=href attr:class=class attr:aria-current=move || if active.get() { "page" } else { "false" }>
            {label}
        </A>
    }
}

#[component]
fn GeneralSection() -> impl IntoView {
    let site = use_site_state();
    let i18n = use_i18n();
    let tr = move |key: &str| {
        i18n.try_with_value(|i| i.t(key))
            .unwrap_or_else(|| key.to_string())
    };
    let tr_with = move |key: &str, args: &[(&str, &str)]| {
        i18n.try_with_value(|i| i.t_with(key, args))
            .unwrap_or_else(|| key.to_string())
    };
    let checking = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let healthy = RwSignal::new(None::<String>);
    let workspaces = RwSignal::new(None::<usize>);
    let entries = RwSignal::new(None::<usize>);

    // Two real calls, in the order that fails first matters: `/health` is the
    // server's own liveness answer (no session needed), and the desk layout is
    // the engine-backed read the sidebar depends on. A failure is shown with
    // the endpoint's own message, never as a green line.
    let check = move |_| {
        checking.set(true);
        error.set(None);
        healthy.set(None);
        workspaces.set(None);
        entries.set(None);
        leptos::task::spawn_local(async move {
            match crate::api::api_get_raw("/health").await {
                Ok(body) => {
                    let status = serde_json::from_str::<serde_json::Value>(&body)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("status")
                                .and_then(|s| s.as_str())
                                .map(str::to_string)
                        })
                        .unwrap_or_else(|| "ok".to_string());
                    healthy.set(Some(status));
                }
                Err(message) => {
                    error.set(Some(tr_with(
                        "views.settings.health_failed",
                        &[("message", &message)],
                    )));
                    checking.set(false);
                    return;
                }
            }
            match session::desk_layout().await {
                Ok(layout) => {
                    workspaces.set(Some(layout.workspaces.len()));
                    entries.set(Some(
                        layout
                            .workspaces
                            .iter()
                            .map(|module| module.items.len())
                            .sum(),
                    ));
                }
                Err(message) => error.set(Some(message)),
            }
            checking.set(false);
        });
    };

    view! {
        <div class="grid grid-cols-1 items-start gap-3 lg:grid-cols-2">
            <Card>
                <CardHeader>
                    <CardTitle>{tr("views.settings.site_title")}</CardTitle>
                    <CardDescription>
                        {tr("views.settings.site_description")}
                    </CardDescription>
                </CardHeader>
                <CardContent class="p-0">
                    <t::Table class="[&_th:first-child]:pl-4 [&_td:first-child]:pl-4 sm:[&_th:first-child]:pl-6 sm:[&_td:first-child]:pl-6 [&_th:last-child]:pr-4 [&_td:last-child]:pr-4 sm:[&_th:last-child]:pr-6 sm:[&_td:last-child]:pr-6">
                        <t::TableBody>
                            <SettingRow label=tr("views.settings.field_name") value=Signal::derive(move || text(&site.with(|s| s.title_prefix.clone()))) />
                            <SettingRow label=tr("views.settings.field_home") value=Signal::derive(move || text(&site.with(|s| s.home_page.clone()))) />
                            <SettingRow label=tr("views.settings.field_language") value=Signal::derive(move || text(&site.with(|s| s.language.clone()))) />
                            <SettingRow label=tr("views.settings.field_help") value=Signal::derive(move || text(&site.with(|s| s.help_link.clone()))) />
                            <SettingRow label=tr("views.settings.field_announcement") value=Signal::derive(move || text(&site.with(|s| s.announcement.clone()))) />
                        </t::TableBody>
                    </t::Table>
                </CardContent>
            </Card>

            <Card>
                <CardHeader>
                    <CardTitle>{tr("views.settings.diagnostics_title")}</CardTitle>
                    <CardDescription>
                        {tr("views.settings.diagnostics_description")}
                    </CardDescription>
                </CardHeader>
                <CardContent class="flex flex-col gap-4">
                    <div class="flex flex-wrap items-center gap-2">
                        <Button
                            variant=ButtonVariant::Default
                            size=ButtonSize::Sm
                            attr:disabled=checking
                            on:click=check
                        >
                            {tr("views.settings.check_server")}
                        </Button>
                        <Show when=move || checking.get()>
                            <span class="text-sm text-muted-foreground">{tr("views.settings.checking")}</span>
                        </Show>
                    </div>

                    <Show when=move || checking.get()>
                        <LoadingLines lines=2 />
                    </Show>
                    <Show when=move || error.get().is_some()>
                        <Feedback kind=FeedbackKind::Error message=Signal::derive(move || error.get()) />
                    </Show>
                    <Show when=move || {
                        !checking.get() && error.get().is_none() && workspaces.get().is_some()
                    }>
                        <Feedback
                            kind=FeedbackKind::Success
                            message=Signal::derive(move || {
                                Some(tr_with(
                                    "views.settings.check_result",
                                    &[
                                        ("status", &healthy.get().unwrap_or_else(|| "ok".to_string())),
                                        ("workspaces", &workspaces.get().unwrap_or_default().to_string()),
                                        ("entries", &entries.get().unwrap_or_default().to_string()),
                                    ],
                                ))
                            })
                        />
                    </Show>
                </CardContent>
            </Card>
        </div>

        <Card>
            <CardHeader>
                <CardTitle>{tr("views.settings.appearance_title")}</CardTitle>
                <CardDescription>
                    {tr("views.settings.appearance_description")}
                </CardDescription>
            </CardHeader>
            <CardContent class="flex flex-wrap items-center gap-3">
                <crate::components::ui::theme_toggle::ThemeToggle />
                <span class="text-sm text-muted-foreground">
                    {tr("views.settings.appearance_hint")}
                </span>
            </CardContent>
        </Card>

        <MobileCard />
    }
}

/// The mobile presentation preferences.
///
/// They are per device, not install-wide: they describe the browser that is
/// looking at the Desk, so they are stored in `localStorage` and applied the
/// moment a checkbox changes — no save button, and nothing sent to the server.
#[component]
fn MobileCard() -> impl IntoView {
    let prefs = use_mobile_prefs();
    let i18n = use_i18n();
    let tr = move |key: &str| {
        i18n.try_with_value(|i| i.t(key))
            .unwrap_or_else(|| key.to_string())
    };
    // Built in the component body, not inside the `view!`: a callback created
    // inside a re-running render closure would be disposed with that closure.
    let set_bottom_nav = Callback::new(move |on: bool| prefs.update(|p| p.bottom_nav = on));
    let set_sidebar_drawer = Callback::new(move |on: bool| prefs.update(|p| p.sidebar_drawer = on));
    let set_form_sidebar = Callback::new(move |on: bool| prefs.update(|p| p.form_sidebar = on));

    view! {
        <Card>
            <CardHeader>
                <CardTitle>{tr("views.settings.mobile_title")}</CardTitle>
                <CardDescription>
                    {tr("views.settings.mobile_description")}
                </CardDescription>
            </CardHeader>
            <CardContent class="flex flex-col gap-4">
                <PrefRow
                    label=tr("views.settings.mobile_bottom_nav")
                    description=tr("views.settings.mobile_bottom_nav_desc")
                    checked=Signal::derive(move || prefs.get().bottom_nav)
                    on_change=set_bottom_nav
                />
                <PrefRow
                    label=tr("views.settings.mobile_sidebar")
                    description=tr("views.settings.mobile_sidebar_desc")
                    checked=Signal::derive(move || prefs.get().sidebar_drawer)
                    on_change=set_sidebar_drawer
                />
                <PrefRow
                    label=tr("views.settings.mobile_form_sidebar")
                    description=tr("views.settings.mobile_form_sidebar_desc")
                    checked=Signal::derive(move || prefs.get().form_sidebar)
                    on_change=set_form_sidebar
                />
            </CardContent>
        </Card>
    }
}

/// One checkbox with its label and the sentence that explains the trade-off.
#[component]
fn PrefRow(
    label: String,
    description: String,
    checked: Signal<bool>,
    on_change: Callback<bool>,
) -> impl IntoView {
    let aria = label.clone();
    view! {
        <div class="flex items-start gap-3">
            <Checkbox
                class="mt-1"
                aria_label=aria
                checked=checked
                on_checked_change=on_change
            />
            <div class="min-w-0">
                <div class="text-sm font-medium">{label}</div>
                <p class="text-sm text-muted-foreground">{description}</p>
            </div>
        </div>
    }
}

/// About: what this build is, stated once and only from values that exist.
///
/// The old card was a "version table" of hardcoded strings (a `Leptos 0.8` that
/// will rot, an "Interfaz" line that says nothing) presented like live data. The
/// crate's own version is the one value that is true by construction, so it is
/// the only one shown; anything about the *server* belongs to the diagnostic
/// above, which actually asks it.
#[component]
fn AboutSection() -> impl IntoView {
    let i18n = use_i18n();
    let tr = move |key: &str| {
        i18n.try_with_value(|i| i.t(key))
            .unwrap_or_else(|| key.to_string())
    };
    view! {
        <Card>
            <CardHeader>
                <CardTitle>{tr("views.settings.about_title")}</CardTitle>
                <CardDescription>
                    {tr("views.settings.about_description")}
                </CardDescription>
            </CardHeader>
            <CardContent class="space-y-2 text-sm">
                <p>
                    <span class="text-muted-foreground">{tr("views.settings.desk_version")}</span>
                    {env!("CARGO_PKG_VERSION")}
                </p>
                <p class="text-muted-foreground">
                    {tr("views.settings.about_note")}
                </p>
            </CardContent>
        </Card>
    }
}

#[component]
fn SettingRow(label: String, value: Signal<String>) -> impl IntoView {
    view! {
        <t::TableRow>
            <t::TableCell class="w-1/3 text-muted-foreground">{label}</t::TableCell>
            <t::TableCell class="break-words">{move || value.get()}</t::TableCell>
        </t::TableRow>
    }
}

fn text(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        "—".to_string()
    } else {
        value.to_string()
    }
}
