//! Re-authentication without leaving the page.
//!
//! A Desk session can expire while a form is open. The old behaviour was a
//! redirect to `/login`, which throws away everything the browser had not yet
//! saved: the form unmounts, the router replaces the route, and the user has to
//! find the document again after signing in. The transport now only *reports*
//! the expiry; this module turns that report into a modal over the live page, so
//! the form state, the route and any half-typed edit survive the round trip.
//!
//! The flag is process-wide on purpose. The 401 is observed by a background
//! task (an API call, not a click handler), where no reactive owner or context
//! exists, so it cannot be published through `provide_context`. One global
//! `ArcRwSignal` gives that task a write handle and gives the shell (mounted
//! once, next to the toaster) the one place that reads it.
//!
//! `POST /api/auth/login` is deliberately excluded by the transport: a 401 from
//! the sign-in endpoint is a wrong password, not an expired session, and
//! reopening this dialog for it would hide the error the login form is showing.

use std::sync::OnceLock;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::client::{api_request_raw, clear_meta_cache, error_message};
use crate::components::ui::button::{Button, ButtonVariant};
use crate::components::ui::dialog::{
    Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle,
};
use crate::components::ui::input::{Input, InputType};
use crate::i18n::{DeskI18n, use_i18n};
use crate::web_user::WebUser;

/// The one process-wide "session expired" flag.
///
/// `ArcRwSignal`, not `RwSignal`: the latter is an arena handle owned by the
/// reactive tree, so a signal created during a request/component render is
/// disposed with that owner and the next reader panics. This one owns its
/// `Arc`, needs no owner, and is safe to create in the transport task (which
/// has no reactive context) as well as in the shell. `OnceLock` makes the first
/// caller the creator whichever side gets there first.
fn relogin_required() -> ArcRwSignal<bool> {
    static REQUIRED: OnceLock<ArcRwSignal<bool>> = OnceLock::new();
    REQUIRED.get_or_init(|| ArcRwSignal::new(false)).clone()
}

/// Mark the session as expired, from the transport.
///
/// Idempotent: several in-flight requests can answer 401 at once, and the
/// dialog must open (and stay open) only once.
pub fn note_unauthorized() {
    relogin_required().set(true);
}

/// The session-expiry modal, mounted once by the Desk shell.
///
/// It reads the current identity for the email prefill and writes the fresh one
/// back on success, so the rest of the chrome (sidebar, user menu) follows the
/// re-authenticated session without a reload.
#[must_use]
#[component]
pub fn ReloginDialog() -> impl IntoView {
    // The dialog's controls need an `RwSignal` (that is what `DialogContent`
    // takes); this handle shares the process-wide Arc, so writing through it in
    // the transport opens the same dialog.
    let open = RwSignal::from(&relogin_required());
    let user = use_context::<RwSignal<WebUser>>();
    // `StoredValue` keeps the translator `Copy`, so the submit callback and the
    // static dialog markup can share it.
    let i18n = use_i18n();
    let tr = move |key: &str| i18n.with_value(|i18n: &DeskI18n| i18n.t(key));
    let tr_with = move |key: &str, args: &[(&str, &str)]| {
        i18n.with_value(|i18n: &DeskI18n| i18n.t_with(key, args))
    };

    let email = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    // The email is prefilled from the live session rather than captured at
    // mount: the shell outlives every login, and the dialog must offer the user
    // who is actually signed in now. Typing is never overwritten, so reopening
    // after a failed attempt keeps what the user entered.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        if email.get_untracked().trim().is_empty()
            && let Some(current) = user.map(|user| user.get_untracked())
            && !current.is_guest()
        {
            email.set(current.email);
        }
    });

    let submit = Callback::new(move |()| {
        if busy.get_untracked() {
            return;
        }
        let username = email.get_untracked().trim().to_string();
        let secret = password.get_untracked();
        if username.is_empty() || secret.is_empty() {
            error.set(Some(tr("login.error.empty_credentials")));
            return;
        }
        error.set(None);
        busy.set(true);
        spawn_local(async move {
            let body = serde_json::json!({
                "username": username,
                "password": secret,
            })
            .to_string();
            match api_request_raw("POST", "/api/auth/login", Some(&body)).await {
                Err(message) => error.set(Some(message)),
                Ok((status, text)) => {
                    if !(200..300).contains(&status) {
                        // 429 is the concurrent-session cap, not a bad
                        // password: saying "credenciales incorrectas" there
                        // would send the user hunting for a problem that is
                        // actually another open tab.
                        let message = if status == 429 {
                            tr_with(
                                "desk.relogin.too_many_sessions",
                                &[("message", &error_message(status, &text))],
                            )
                        } else {
                            error_message(status, &text)
                        };
                        error.set(Some(message));
                    } else if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                        if value
                            .get("mfa_required")
                            .and_then(serde_json::Value::as_bool)
                            == Some(true)
                        {
                            error.set(Some(tr("desk.relogin.mfa_required")));
                        } else {
                            match serde_json::from_value::<WebUser>(value) {
                                Ok(me) => {
                                    if let Some(user) = user {
                                        user.set(me);
                                    }
                                    clear_meta_cache();
                                    password.set(String::new());
                                    error.set(None);
                                    open.set(false);
                                }
                                Err(e) => error.set(Some(e.to_string())),
                            }
                        }
                    } else {
                        error.set(Some(tr("desk.relogin.invalid_response")));
                    }
                }
            }
            busy.set(false);
        });
    });

    // Enter in either field submits, exactly like the sign-in page.
    let on_enter = move |event: leptos::ev::KeyboardEvent| {
        if event.key() == "Enter" {
            submit.run(());
        }
    };

    view! {
        <Dialog>
            <DialogContent
                open=open
                class="max-w-sm"
                show_close_button=false
                close_on_backdrop_click=false
            >
                <DialogHeader>
                    <DialogTitle>{tr("desk.relogin.title")}</DialogTitle>
                    <DialogDescription>
                        {tr("desk.relogin.description")}
                    </DialogDescription>
                </DialogHeader>

                <div class="mt-4 flex flex-col gap-3" on:keydown=on_enter>
                    <div class="flex flex-col gap-1">
                        <span class="text-xs font-medium text-muted-foreground">
                            {tr("login.email")}
                        </span>
                        <Input
                            r#type=InputType::Text
                            aria_label=tr("login.email")
                            autocomplete="username"
                            bind_value=email
                        />
                    </div>
                    <div class="flex flex-col gap-1">
                        <span class="text-xs font-medium text-muted-foreground">{tr("login.password")}</span>
                        <Input
                            r#type=InputType::Password
                            aria_label=tr("login.password")
                            autocomplete="current-password"
                            bind_value=password
                        />
                    </div>

                    <Show when=move || error.get().is_some()>
                        <p
                            role="alert"
                            class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
                        >
                            {move || error.get().unwrap_or_default()}
                        </p>
                    </Show>

                    <div class="flex justify-end">
                        <Button
                            variant=ButtonVariant::Default
                            attr:disabled=move || busy.get()
                            on:click=move |_| submit.run(())
                        >
                            {move || if busy.get() { tr("desk.relogin.submitting") } else { tr("login.submit") }}
                        </Button>
                    </div>
                </div>
            </DialogContent>
        </Dialog>
    }
}
