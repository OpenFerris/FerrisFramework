//! Standalone sign-in page.
//!
//! The login used to be an overlay drawn on top of the Desk, so an
//! unauthenticated deep link still rendered the whole desk behind it and every
//! view fired an unauthenticated request. It is now its own route (`/login`):
//! the server redirects an unauthenticated desk request here (carrying the page
//! they wanted in `?next=`), and a successful sign-in sends them back. The
//! [`crate::app`] module is what renders it outside the desk chrome.

use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::{use_navigate, use_query_map};

use crate::api::drive;
use crate::app::LoginComplete;
use crate::components::ui::button::{Button, ButtonVariant};
use crate::desk::session as desk_session;
use crate::desk::session::LoginOutcome;
use crate::views::views_components::{Feedback, FeedbackKind};
use crate::web_user::WebUser;

#[component]
pub fn LoginView(on_login: impl Fn(WebUser) + Clone + Send + Sync + 'static) -> impl IntoView {
    let (email, set_email) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (error, set_error) = signal(None::<String>);
    let (busy, set_busy) = signal(false);
    // The MFA step: `Some(challenge)` only after the password checked out on an
    // account that owes a second factor.
    let (challenge, set_challenge) = signal(None::<String>);
    let (code, set_code) = signal(String::new());
    // Providers are optional: the deployment may configure none, and the OAuth
    // surface may not even be mounted. The list is read once on the browser.
    let (providers, set_providers) = signal(Vec::<String>::new());
    Effect::new(move |_| {
        drive(async move {
            if let Ok(list) = desk_session::oauth_providers().await {
                set_providers.set(list);
            }
        });
    });

    let on_submit = {
        let on_login = on_login.clone();
        move || {
            if busy.get_untracked() {
                return;
            }
            let username = email.get_untracked();
            let password = password.get_untracked();
            if username.trim().is_empty() || password.is_empty() {
                set_error.set(Some("Escribe tu correo y contraseña.".to_string()));
                return;
            }
            set_error.set(None);
            set_busy.set(true);
            let on_login = on_login.clone();
            drive(async move {
                match desk_session::login(username.trim(), &password).await {
                    Ok(LoginOutcome::SignedIn(me)) => on_login(me),
                    Ok(LoginOutcome::MfaRequired { challenge }) => {
                        set_code.set(String::new());
                        set_challenge.set(Some(challenge));
                    }
                    Err(e) => set_error.set(Some(e)),
                }
                set_busy.set(false);
            });
        }
    };

    let on_verify = {
        let on_login = on_login.clone();
        move || {
            if busy.get_untracked() {
                return;
            }
            let Some(challenge) = challenge.get_untracked() else {
                return;
            };
            let code = code.get_untracked();
            if code.trim().is_empty() {
                set_error.set(Some(
                    "Escribe el código de tu aplicación de autenticación.".to_string(),
                ));
                return;
            }
            set_error.set(None);
            set_busy.set(true);
            let on_login = on_login.clone();
            drive(async move {
                match desk_session::verify_mfa(&challenge, code.trim()).await {
                    Ok(me) => on_login(me),
                    Err(e) => set_error.set(Some(e)),
                }
                set_busy.set(false);
            });
        }
    };

    let on_email_enter = on_submit.clone();
    let on_password_enter = on_submit.clone();
    let on_button = on_submit.clone();
    let on_code_enter = on_verify.clone();
    let on_verify_button = on_verify.clone();

    view! {
        <div class="min-h-dvh flex items-center justify-center bg-muted">
            <div class="w-full max-w-sm mx-4">
                <div class="bg-card text-card-foreground rounded-xl shadow-sm border border-border p-6">
                    <div class="flex items-center gap-3 mb-6">
                        <div class="ferris-login__logo w-10 h-10 rounded-lg flex items-center justify-center">
                            <span class="font-bold text-xl">"F"</span>
                        </div>
                        <div>
                            <h1 class="text-lg font-bold text-foreground">"Ferris Desk"</h1>
                            <p class="text-xs text-muted-foreground">
                                {move || if challenge.get().is_some() {
                                    "Introduce tu código de doble factor"
                                } else {
                                    "Inicia sesión para continuar"
                                }}
                            </p>
                        </div>
                    </div>

                    <div class="space-y-4">
                        {move || {
                            // The event handlers below are `move` closures, so
                            // they would consume these out of this reactive
                            // block (making it `FnOnce`); hand each a clone.
                            let on_code_enter = on_code_enter.clone();
                            let on_email_enter = on_email_enter.clone();
                            let on_password_enter = on_password_enter.clone();
                            if challenge.get().is_some() {
                                view! {
                                    <div>
                                        <label class="block text-sm font-medium text-foreground mb-1" for="login-code">
                                            "Código de verificación"
                                        </label>
                                        <input
                                            id="login-code"
                                            type="text"
                                            inputmode="numeric"
                                            autocomplete="one-time-code"
                                            placeholder="123456"
                                            class="ferris-input h-10 w-full rounded-md border px-3 py-2 text-base md:text-sm"
                                            prop:value=code
                                            on:input=move |ev| set_code.set(event_target_value(&ev))
                                            on:keydown=move |ev| if ev.key() == "Enter" { on_code_enter() }
                                        />
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div>
                                        <label class="block text-sm font-medium text-foreground mb-1" for="login-email">
                                            "Correo o usuario"
                                        </label>
                                        <input
                                            id="login-email"
                                            type="text"
                                            autocomplete="username"
                                            placeholder="admin@local"
                                            class="ferris-input h-10 w-full rounded-md border px-3 py-2 text-base md:text-sm"
                                            prop:value=email
                                            on:input=move |ev| set_email.set(event_target_value(&ev))
                                            on:keydown=move |ev| if ev.key() == "Enter" { on_email_enter() }
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium text-foreground mb-1" for="login-password">
                                            "Contraseña"
                                        </label>
                                        <input
                                            id="login-password"
                                            type="password"
                                            autocomplete="current-password"
                                            placeholder="••••••••"
                                            class="ferris-input h-10 w-full rounded-md border px-3 py-2 text-base md:text-sm"
                                            prop:value=password
                                            on:input=move |ev| set_password.set(event_target_value(&ev))
                                            on:keydown=move |ev| if ev.key() == "Enter" { on_password_enter() }
                                        />
                                    </div>
                                }.into_any()
                            }
                        }}

                        <Feedback
                            kind=FeedbackKind::Error
                            message=Signal::derive(move || error.get())
                        />

                        <Button
                            variant=ButtonVariant::Default
                            class="w-full"
                            attr:disabled=move || busy.get()
                            on:click=move |_| {
                                if challenge.get_untracked().is_some() {
                                    on_verify_button();
                                } else {
                                    on_button();
                                }
                            }
                        >
                            {move || {
                                if busy.get() {
                                    if challenge.get().is_some() { "Verificando…" } else { "Entrando…" }
                                } else if challenge.get().is_some() {
                                    "Verificar"
                                } else {
                                    "Entrar"
                                }
                            }}
                        </Button>

                        {move || {
                            (!providers.get().is_empty() && challenge.get().is_none()).then(|| {
                                let list = providers.get();
                                view! {
                                    <div class="mt-4 flex flex-col gap-3">
                                        <div class="flex items-center gap-2 text-xs text-muted-foreground">
                                            <span class="h-px flex-1 bg-border"></span>
                                            "o continúa con"
                                            <span class="h-px flex-1 bg-border"></span>
                                        </div>
                                        {list.into_iter().map(|provider| {
                                            let label = provider_label(&provider);
                                            let href = format!(
                                                "/api/oauth/{}/login?next=/desk",
                                                provider
                                            );
                                            view! {
                                                <Button
                                                    variant=ButtonVariant::Outline
                                                    class="w-full"
                                                    href=href
                                                >
                                                    {label}
                                                </Button>
                                            }
                                        }).collect_view()}
                                    </div>
                                }
                            })
                        }}

                        {move || {
                            challenge.get().is_some().then(|| {
                                view! {
                                    <button
                                        type="button"
                                        class="inline-flex min-h-10 w-full items-center justify-center rounded-md text-center text-xs text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
                                        on:click=move |_| {
                                            set_challenge.set(None);
                                            set_error.set(None);
                                        }
                                    >
                                        "Volver al inicio de sesión"
                                    </button>
                                }
                            })
                        }}
                    </div>
                </div>
                {cfg!(debug_assertions).then(|| view! {
                    <p class="mt-4 text-center text-xs text-muted-foreground">
                        "Cuenta de desarrollo local: admin@local / ferrisAdmin123"
                    </p>
                })}
            </div>
        </div>
    }
}

/// A readable label for a provider id (`google` → `Google`).
fn provider_label(provider: &str) -> String {
    let mut chars = provider.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The `/login` route view: the sign-in form plus the post-login hop back to
/// whatever the visitor originally asked for (`?next=`).
#[component]
pub fn LoginPage() -> impl IntoView {
    let navigate = use_navigate();
    let query = use_query_map();
    let complete = expect_context::<LoginComplete>();

    let on_login = move |me: WebUser| {
        complete.0.run(me);
        let target = safe_next(query.get_untracked().get("next"));
        navigate(
            &target,
            NavigateOptions {
                replace: true,
                ..Default::default()
            },
        );
    };

    view! { <LoginView on_login=on_login /> }
}

/// The `next` destination, honoured only when it is a local path.
///
/// A login page that redirected anywhere a request asked would be an open
/// redirect; `//evil.example` (protocol-relative) and absolute URLs are refused
/// and the desk home is used instead.
fn safe_next(next: Option<String>) -> String {
    match next {
        Some(path)
            if path.starts_with('/')
                && !path.starts_with("//")
                && !path.contains('\\')
                && !path.contains('\n')
                && !path.contains('\r') =>
        {
            path
        }
        _ => "/".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::safe_next;

    #[test]
    fn safe_next_accepts_local_paths_only() {
        assert_eq!(safe_next(Some("/desk/app/Task".into())), "/desk/app/Task");
        assert_eq!(safe_next(Some("/".into())), "/");
        assert_eq!(safe_next(Some("/desk?x=1".into())), "/desk?x=1");
        // Open-redirect attempts fall back to the desk home.
        assert_eq!(safe_next(Some("//evil.example".into())), "/");
        assert_eq!(safe_next(Some("https://evil.example".into())), "/");
        assert_eq!(safe_next(Some("/\\evil".into())), "/");
        assert_eq!(safe_next(Some("/x\nSet-Cookie: y".into())), "/");
        assert_eq!(safe_next(None), "/");
    }
}
