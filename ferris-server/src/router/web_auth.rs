//! Server-side auth gate in front of the Leptos desk routes.

use axum::{
    extract::{FromRequestParts, Query as AxumQuery, Request, State},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use ferris_auth::middleware::OptionalUser;
use ferris_auth::{AuthState, User, UserType};
use ferris_core::I18n;
use ferris_core::i18n::Locale;
use ferris_db::Db;
use ferris_schema::contract::identity::WebUser;
use ferris_web::i18n::DeskI18n;
use serde::Deserialize;

use super::pages::page_path_matches;
use super::security::CspNonce;

/// `?next=` exactly as it arrives on `/login`.
#[derive(Debug, Deserialize)]
struct NextQuery {
    next: Option<String>,
}

/// The identity [`web_auth_guard`] resolved for this request, stashed in request
/// extensions so the SSR render can paint the real user instead of a guest.
/// `engine` is the full auth-stack user; `web` is the narrower projection the
/// browser is allowed to see (and the one `/api/auth/session` returns).
#[derive(Clone)]
struct DeskIdentity {
    engine: User,
    web: WebUser,
}

/// Whether `path` is one of the Desk's own (core) routes, which always require a
/// session. Assets and the plugin/website pages the router also serves are not
/// decided here — see [`requires_desk_auth`].
pub(super) fn is_core_desk_path(path: &str) -> bool {
    path == "/"
        || path == "/desk"
        || path.starts_with("/desk/")
        || path.starts_with("/report/")
        || path.starts_with("/kanban/")
        || path.starts_with("/calendar")
        || path.starts_with("/settings")
}

/// Whether a loaded plugin declares a *dashboard* at `path` (e.g. SolmecBOX's
/// `/planta`, `/mantenimiento`). A dashboard mounts the same desk widgets the
/// home grid does, so it is desk UI and needs a session exactly like the core
/// routes; a plugin's *website* pages stay public.
fn is_dashboard_page(path: &str) -> bool {
    ferris_core::ui_extensions::registered_ui()
        .iter()
        .any(|app| {
            app.dashboards
                .iter()
                .any(|dashboard| page_path_matches(&dashboard.route, path))
        })
}

/// Whether `path` is part of the Desk proper, which requires a session: a core
/// desk route or a plugin dashboard. Website pages, assets and `/login` pass
/// through. The plugin registry is read live, so a dashboard added by a hot
/// reload is gated on its next request without a restart.
fn requires_desk_auth(path: &str) -> bool {
    is_core_desk_path(path) || is_dashboard_page(path)
}

/// Whether `next` is a path on this origin the login page may bounce back to.
/// Only an absolute path (`/…`) that cannot be re-read as a scheme- or
/// authority-relative URL qualifies; everything else falls back to the desk
/// home, so a crafted `?next=` cannot turn login into an open redirect.
///
/// The check runs on the normalized value, because a browser parses the raw
/// one with the WHATWG rules while we compare characters: the parser strips
/// tabs and newlines *anywhere* (`"/\t/evil.example"` is read as
/// `"//evil.example"`, i.e. an authority) and reads `\` as `/`. Rejecting the
/// characters up front and trimming surrounding ASCII whitespace keeps the
/// comparison on the string the browser will actually follow.
pub(super) fn is_local_path(target: &str) -> bool {
    if target
        .chars()
        .any(|c| matches!(c, '\\' | '\n' | '\r' | '\t'))
    {
        return false;
    }
    let normalized = target.trim();
    normalized.starts_with('/') && !normalized.starts_with("//")
}

/// Percent-encode a query value (RFC 3986 unreserved characters pass through).
/// Space is `%20`, which the `?next=` decoding accepts either way.
pub(super) fn encode_query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Project the auth-stack [`User`] onto the browser-visible [`WebUser`]. The
/// `user_type` is spelled out to match what `UserStore::public_json` serializes,
/// which is what `WebUser::is_guest` and the client read.
fn engine_to_web_user(user: &User) -> WebUser {
    WebUser {
        id: user.id.clone(),
        email: user.email.clone(),
        full_name: user.full_name.clone(),
        username: user.username.clone(),
        roles: user.roles.clone(),
        user_type: match user.user_type {
            UserType::SystemUser => "SystemUser",
            UserType::WebsiteUser => "WebsiteUser",
            UserType::Guest => "Guest",
        }
        .to_string(),
        language: user.language.clone(),
    }
}

/// Server-side auth gate in front of the Leptos router.
///
/// A desk request with no session — a core desk route or a plugin **dashboard**
/// such as `/planta` — is answered with a `303 See Other` to the standalone
/// `/login` route, carrying `?next=` back to where the visitor was headed; a
/// request for `/login` that already carries a session is bounced to that same
/// `next` (or the desk home). Assets, API routes and plugin *website* pages pass
/// straight through. On the way through, the caller is resolved once — with the
/// same `OptionalUser` extractor the plugin layer uses — and stashed in request
/// extensions for `provide_desk_context` to pick up.
pub(super) async fn web_auth_guard(
    State(auth): State<AuthState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let is_login = path == ferris_web::app::LOGIN_PATH;
    if !is_login && !requires_desk_auth(&path) {
        return next.run(request).await;
    }

    // Read the query before the request is split into parts.
    let query = request.uri().query().unwrap_or_default().to_string();
    let requested_next = if is_login {
        AxumQuery::<NextQuery>::try_from_uri(request.uri())
            .ok()
            .and_then(|query| query.0.next)
            .filter(|target| is_local_path(target))
    } else {
        None
    };

    let (mut parts, body) = request.into_parts();
    let identity = OptionalUser::from_request_parts(&mut parts, &auth)
        .await
        .map(|extractor| extractor.0)
        .unwrap_or_else(|never| match never {})
        .map(|engine| DeskIdentity {
            web: engine_to_web_user(&engine),
            engine,
        });
    if let Some(identity) = &identity {
        parts.extensions.insert(identity.clone());
    }
    let request = Request::from_parts(parts, body);

    if is_login {
        // Already signed in: never show the form again.
        if identity.is_some() {
            return Redirect::to(requested_next.as_deref().unwrap_or("/")).into_response();
        }
        return next.run(request).await;
    }

    match identity {
        Some(_) => next.run(request).await,
        None => {
            let original = if query.is_empty() {
                path
            } else {
                format!("{path}?{query}")
            };
            let target = format!(
                "{}?next={}",
                ferris_web::app::LOGIN_PATH,
                encode_query_value(&original)
            );
            Redirect::to(&target).into_response()
        }
    }
}

/// Scope the identity [`web_auth_guard`] resolved as the engine's ambient user
/// for the SSR render.
///
/// The desk bridge (`DocStore::Server`) reads documents through
/// `ferris_api::DocStore`, which authorises `ferris_document::current_user()` —
/// not a parameter. Without this scope the SSR pass ran as an anonymous caller,
/// so a page the browser would hydrate with data pre-rendered `403`/empty,
/// breaking the SSR/hydrate parity the Desk promises. No session stays
/// anonymous on purpose: the engine fails closed.
///
/// Mounted *inside* the guard (see `build_router`): the guard is the outer
/// layer, so the `DeskIdentity` it inserts into the request extensions is
/// already there when this layer runs, and the cookie/bearer resolution happens
/// once per request.
pub(super) async fn bind_desk_user(request: Request, next: Next) -> Response {
    let user = desk_engine_user(request.extensions());
    ferris_document::with_user(user, next.run(request)).await
}

/// The auth-stack user [`web_auth_guard`] resolved for this request, if any.
///
/// Kept separate from the middleware so a test can prove the extension is what
/// reaches `ferris_document`'s ambient context without building a `Next`.
fn desk_engine_user(extensions: &axum::http::Extensions) -> Option<User> {
    extensions
        .get::<DeskIdentity>()
        .map(|identity| identity.engine.clone())
}

/// Provide the authenticated identity (or the guest) and the data pool as SSR
/// context for the Desk. The identity is the one [`web_auth_guard`] resolved;
/// a render that did not go through the guard falls back to the guest, which is
/// exactly what `/login` renders as.
pub(super) fn provide_desk_context(pool: Db, i18n: &I18n) {
    use ferris_auth::User;
    use leptos::prelude::{provide_context, use_context};

    // The request `Parts` carry what the guard resolved; `leptos_axum` provides
    // them for exactly this kind of layered context.
    let parts = use_context::<axum::http::request::Parts>();
    let identity = parts
        .as_ref()
        .and_then(|parts| parts.extensions.get::<DeskIdentity>().cloned());

    // Negotiate once per render. The catalogue travels in the context (SSR)
    // and in the bootstrap `<meta>` (hydration), so the client never refetches.
    let preferred = identity
        .as_ref()
        .and_then(|identity| identity.engine.language.as_deref())
        .and_then(Locale::parse)
        .filter(|locale| i18n.supported().contains(locale));
    let locale = preferred.unwrap_or_else(|| {
        parts
            .as_ref()
            .and_then(|parts| parts.headers.get(axum::http::header::ACCEPT_LANGUAGE))
            .and_then(|value| value.to_str().ok())
            .map_or_else(
                || i18n.default_locale().clone(),
                |tag| i18n.negotiate(Some(tag)),
            )
    });
    let catalog = i18n.catalog_json(&locale);
    let supported = i18n
        .supported()
        .iter()
        .map(|locale| locale.as_str().to_string())
        .collect();
    provide_context(DeskI18n::from_catalog(locale.as_str(), &catalog).with_supported(supported));
    // The SSR bridge reads these to project metadata (DocType/field/workspace
    // labels) in the same language as the shell, so the server-rendered page
    // and the hydrated one agree on every label.
    provide_context(i18n.clone());
    provide_context(locale.clone());

    match identity {
        Some(identity) => {
            provide_context(identity.web);
            provide_context(identity.engine);
        }
        None => {
            provide_context(WebUser::guest());
            provide_context(User::guest());
        }
    }
    provide_context(pool);

    // Override the random nonce `leptos_axum` proved earlier with the one the
    // `security_headers` middleware minted and put on the request, so every
    // inline `<script>` it stamps matches the `Content-Security-Policy` header
    // (which is written outside the render and can't know Leptos' nonce). This
    // provider runs *after* `leptos_axum`'s, so the value here wins.
    if let Some(parts) = use_context::<axum::http::request::Parts>()
        && let Some(nonce) = parts.extensions.get::<CspNonce>()
    {
        provide_context(leptos::nonce::Nonce::from_value(nonce.0.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::{DeskIdentity, desk_engine_user, engine_to_web_user, is_local_path};
    use ferris_auth::User;

    /// The WHATWG parser strips raw tabs and newlines before reading a URL, so
    /// `"/\t/evil.example"` becomes `"//evil.example"` — an authority. The
    /// comparison has to reject those characters instead of trusting the
    /// leading `//` check alone.
    #[test]
    fn is_local_path_rejects_parser_normalized_offsite_targets() {
        assert!(!is_local_path("/\t/evil.example"));
        assert!(!is_local_path("//evil.example"));
        assert!(!is_local_path("/\n/evil.example"));
        assert!(!is_local_path("/\r/evil.example"));
        assert!(!is_local_path(" /\t/evil.example "));
        assert!(!is_local_path("/\\evil.example"));
    }

    #[test]
    fn is_local_path_accepts_same_origin_paths() {
        assert!(is_local_path("/"));
        assert!(is_local_path("/desk/core/Task?filter=open"));
        assert!(is_local_path("/desk/a//b"));
        assert!(is_local_path("  /desk/core/Task  "));
    }

    /// The SSR bridge reads the engine as the ambient user (`current_user()`),
    /// not through a parameter. This pins the contract of the layer: the
    /// identity the guard resolved is what `with_user` scopes during render.
    #[tokio::test]
    async fn an_authenticated_desk_request_scopes_the_engine_user() {
        let mut user = User::system_user("user-1".to_string(), "Ada".to_string());
        user.roles = vec!["System Manager".to_string()];

        let mut extensions = axum::http::Extensions::new();
        extensions.insert(DeskIdentity {
            web: engine_to_web_user(&user),
            engine: user.clone(),
        });

        let scoped = desk_engine_user(&extensions);
        let seen =
            ferris_document::with_user(scoped, async { ferris_document::current_user() }).await;

        assert_eq!(seen.map(|user| user.id), Some(user.id));
    }

    /// No session means no ambient user, which the engine treats as anonymous
    /// and fails closed on; it must not inherit a stale identity.
    #[tokio::test]
    async fn a_guest_desk_request_stays_anonymous() {
        let extensions = axum::http::Extensions::new();
        let scoped = desk_engine_user(&extensions);
        let seen =
            ferris_document::with_user(scoped, async { ferris_document::current_user() }).await;
        assert!(seen.is_none());
    }
}
