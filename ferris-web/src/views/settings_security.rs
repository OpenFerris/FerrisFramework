//! Settings → Seguridad: self-service MFA enrolment.
//!
//! The backend owns the flow (`/api/auth/mfa/{enable,activate,disable}`): this
//! screen starts an enrolment, shows the secret/URI and the one-time backup
//! codes, and confirms the first code. No QR image is generated (the server
//! returns the `otpauth://` URI; every authenticator app accepts it pasted) and
//! the backup codes are only ever shown here, once.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::DocStore;
use crate::components::ui::button::{Button, ButtonSize, ButtonVariant};
use crate::components::ui::card::{Card, CardContent, CardDescription, CardHeader, CardTitle};
use crate::components::ui::input::{Input, InputType};
use crate::desk::session::{self as desk_session, MfaEnrollment};
use crate::i18n::use_i18n;
use crate::views::views_components::{Feedback, FeedbackKind, LoadingLines};

#[component]
pub fn SecuritySection() -> impl IntoView {
    let i18n = use_i18n();
    let t = move |key: &str| {
        i18n.try_with_value(|i18n| i18n.t(key))
            .unwrap_or_else(|| key.to_string())
    };
    let loading = RwSignal::new(true);
    let busy = RwSignal::new(false);
    let enabled = RwSignal::new(false);
    let user_label = RwSignal::new(String::new());
    let enrollment = RwSignal::new(None::<MfaEnrollment>);
    let code = RwSignal::new(String::new());
    let disable_code = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let notice = RwSignal::new(None::<String>);

    // The current user's `mfa_enabled` flag lives on their User document; the
    // session payload has no MFA field.
    let load = move || {
        loading.set(true);
        spawn_local(async move {
            if let Ok(Some(me)) = desk_session::me().await {
                user_label.set(me.full_name.clone());
                if let Ok(store) = DocStore::runtime()
                    && let Ok(doc) = store.get("User", &me.id).await
                {
                    enabled.set(
                        doc.get("mfa_enabled")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                    );
                }
            }
            loading.set(false);
        });
    };
    Effect::new(move |_| load());

    let begin = move || {
        error.set(None);
        notice.set(None);
        busy.set(true);
        let account = user_label.get_untracked();
        spawn_local(async move {
            match desk_session::mfa_enable(Some(&account)).await {
                Ok(found) => {
                    busy.set(false);
                    enrollment.set(Some(found));
                }
                Err(message) => {
                    busy.set(false);
                    error.set(Some(message));
                }
            }
        });
    };

    let activate = move || {
        let value = code.get_untracked().trim().to_string();
        if value.is_empty() {
            error.set(Some(t("views.security.code_required")));
            return;
        }
        error.set(None);
        notice.set(None);
        busy.set(true);
        spawn_local(async move {
            match desk_session::mfa_activate(&value).await {
                Ok(()) => {
                    busy.set(false);
                    enrollment.set(None);
                    code.set(String::new());
                    enabled.set(true);
                    notice.set(Some(t("views.security.activated")));
                }
                Err(message) => {
                    busy.set(false);
                    error.set(Some(message));
                }
            }
        });
    };

    let disable = move || {
        let value = disable_code.get_untracked().trim().to_string();
        if value.is_empty() {
            error.set(Some(t("views.security.disable_code_required")));
            return;
        }
        error.set(None);
        notice.set(None);
        busy.set(true);
        spawn_local(async move {
            match desk_session::mfa_disable(&value).await {
                Ok(()) => {
                    busy.set(false);
                    enabled.set(false);
                    disable_code.set(String::new());
                    notice.set(Some(t("views.security.deactivated")));
                }
                Err(message) => {
                    busy.set(false);
                    error.set(Some(message));
                }
            }
        });
    };

    view! {
        <Card>
            <CardHeader>
                <CardTitle>{t("views.security.title")}</CardTitle>
                <CardDescription>
                    {t("views.security.description")}
                </CardDescription>
            </CardHeader>
            <CardContent>
                <div class="flex flex-col gap-3">
                    <Feedback kind=FeedbackKind::Error message=Signal::derive(move || error.get()) />
                    <Feedback
                        kind=FeedbackKind::Success
                        message=Signal::derive(move || notice.get())
                    />

                    <Show
                        when=move || !loading.get()
                        fallback=|| view! { <LoadingLines lines=2 /> }
                    >
                        <Show
                            when=move || enabled.get()
                            fallback=move || {
                                view! {
                                    <Show
                                        when=move || enrollment.get().is_some()
                                        fallback=move || {
                                            view! {
                                                <p class="text-sm text-muted-foreground">
                                                    {t("views.security.not_enabled")}
                                                </p>
                                                <div>
                                                    <Button
                                                        variant=ButtonVariant::Default
                                                        size=ButtonSize::Sm
                                                        attr:disabled=move || busy.get()
                                                        on:click=move |_| begin()
                                                    >
                                                        {t("views.security.enable")}
                                                    </Button>
                                                </div>
                                            }
                                        }
                                    >
                                        {move || {
                                            enrollment
                                                .get()
                                                .map(|pending| {
                                                    view! {
                                                        <div class="flex flex-col gap-3 rounded-lg border bg-muted/40 p-3">
                                                            <p class="text-sm">
                                                                {t("views.security.step_add")}
                                                            </p>
                                                            <code class="block overflow-x-auto rounded bg-background px-2 py-1 text-xs">
                                                                {pending.secret.clone()}
                                                            </code>
                                                            <code class="block overflow-x-auto rounded bg-background px-2 py-1 text-xs text-muted-foreground">
                                                                {pending.otpauth_uri.clone()}
                                                            </code>
                                                            <p class="text-sm">
                                                                {t("views.security.step_codes")}
                                                            </p>
                                                            <ul class="grid grid-cols-2 gap-1 font-mono text-xs">
                                                                {pending
                                                                    .backup_codes
                                                                    .iter()
                                                                    .map(|code| view! {
                                                                        <li class="rounded bg-background px-2 py-1">{code.clone()}</li>
                                                                    })
                                                                    .collect_view()}
                                                            </ul>
                                                            <p class="text-sm">{t("views.security.step_code")}</p>
                                                            <div class="flex flex-wrap items-center gap-2">
                                                                <Input
                                                                    id="mfa-activate-code"
                                                                    r#type=InputType::Text
                                                                    placeholder="123456"
                                                                    prop:value=move || code.get()
                                                                    on_input=Callback::new(move |text: String| code.set(text))
                                                                />
                                                                <Button
                                                                    variant=ButtonVariant::Default
                                                                    size=ButtonSize::Sm
                                                                    attr:disabled=move || busy.get()
                                                                    on:click=move |_| activate()
                                                                >
                                                                    {t("views.common.confirm")}
                                                                </Button>
                                                            </div>
                                                        </div>
                                                    }
                                                })
                                            }
                                        }
                                    </Show>
                                }
                            }
                        >
                            <p class="text-sm text-muted-foreground">
                                {t("views.security.enabled_hint")}
                            </p>
                            <div class="flex flex-wrap items-center gap-2">
                                <Input
                                    id="mfa-disable-code"
                                    r#type=InputType::Text
                                    placeholder=t("views.security.code_placeholder")
                                    prop:value=move || disable_code.get()
                                    on_input=Callback::new(move |text: String| disable_code.set(text))
                                />
                                <Button
                                    variant=ButtonVariant::Destructive
                                    size=ButtonSize::Sm
                                    attr:disabled=move || busy.get()
                                    on:click=move |_| disable()
                                >
                                    {t("views.security.disable")}
                                </Button>
                            </div>
                        </Show>
                    </Show>
                </div>
            </CardContent>
        </Card>
    }
}
