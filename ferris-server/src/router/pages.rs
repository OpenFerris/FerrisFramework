//! Plugin-declared desk/website pages and the public CMS/forms stored in the
//! database: the live catch-all guard that serves them on a direct load.
//!
//! There are two route sources with very different lifetimes:
//!
//! * the **live registry** (`ferris_core::ui_extensions`) a plugin populates in
//!   `describe()`, which a dev hot-reload can change without a restart — it is
//!   re-read on every request and never cached;
//! * the **stored** `Web Page` / `Web Form` / `Dashboard` documents an
//!   administrator edits in the Desk. They have no boot-time registration, so
//!   before this module the only way to reach one was client-side navigation:
//!   a direct load of its route fell into `not_found_fallback` (FRA-09/FRA-18).
//!
//! The guard below resolves both and keeps the failure semantics intact: a
//! path nothing claims still takes the exact `not_found_fallback` it would have
//! taken without the splat route (JSON for API-shaped callers, the small HTML
//! page for browsers). A published page is server-rendered HTML with its stored
//! content sanitised; a draft is a 404, never a preview.

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Form, FromRequest, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use ferris_auth::RateLimiter;
use ferris_core::{DocumentError, Error};
use ferris_web::website::{PublicRoute, WebForm, WebPage, normalize_route, resolve_public_route};
use tracing::warn;

use super::fallback::not_found_fallback;
use super::state::AppState;

/// Submissions allowed per caller and form inside the limiter window.
const FORM_SUBMISSIONS: u32 = 10;
/// Rate-limit window for public form submissions, in seconds.
const FORM_WINDOW_SECONDS: u64 = 120;

/// One limiter for every public form: keyed per form and caller, so a flood on
/// `/contact` cannot exhaust `/apply`.
static FORM_LIMITER: std::sync::LazyLock<RateLimiter> =
    std::sync::LazyLock::new(|| RateLimiter::new(FORM_SUBMISSIONS, FORM_WINDOW_SECONDS));

/// Fase 2: the axum paths for the dashboards and website pages the loaded
/// plugins declare. The registry (`ferris_core::ui_extensions`) is populated
/// when each plugin's `describe()` registers — in the dev flow that happens
/// before this router is built, so the routes are known at boot.
pub(super) fn plugin_page_paths() -> Vec<String> {
    let mut paths = Vec::new();
    for app in ferris_core::ui_extensions::registered_ui() {
        for dashboard in &app.dashboards {
            paths.push(axum_page_path(&dashboard.route));
        }
        if let Some(website) = &app.website {
            for route in &website.routes {
                paths.push(axum_page_path(&route.path));
            }
        }
    }
    paths
}

/// A declared plugin path (`/blog/:slug`) in axum's pattern syntax
/// (`/blog/{slug}`); empty segments collapse and a bare `"/"` stays `"/"`.
/// Already-converted segments (`{slug}`) pass through untouched.
pub(super) fn axum_page_path(declared: &str) -> String {
    let segments: Vec<String> = declared
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|segment| {
            if let Some(name) = segment.strip_prefix(':') {
                format!("{{{name}}}")
            } else if let Some(name) = segment.strip_prefix('*') {
                format!("{{*{name}}}")
            } else {
                segment.to_string()
            }
        })
        .collect();
    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

/// Fase 3: the guarded `/{*rest}` handler. A path the live registry declares
/// right now — a plugin's dashboard or website page, including one
/// registered after boot — runs the Leptos handler and SSRs the shell (the
/// client finishes resolution with its own catch-all). Everything else gets
/// the exact `not_found_fallback` it would have hit without this route:
/// JSON for API-shaped callers, the small HTML page for browsers — and the
/// outermost socket.io layer still sees `/socket.io` before any routing
/// runs (riesgo R2: the guard sits inside every global layer, and is the
/// only thing the splat route added).
pub(super) async fn live_page_guard(request: Request, next: Next) -> Response {
    if is_declared_page(request.uri().path()) {
        next.run(request).await
    } else {
        not_found_fallback(request).await
    }
}

/// The guard the web router mounts: stored routes first, then the registry-only
/// guard above.
///
/// Kept apart from [`live_page_guard`] so the public-site lookup needs
/// `AppState` (the database) while the registry rule stays testable on its own
/// and unchanged. A route that is neither stored nor declared still ends in
/// `live_page_guard`, which is what preserves the fallback contract.
pub(super) async fn public_page_guard(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    if !is_declared_page(&path) && could_be_stored_route(&path) {
        match resolve_public_route(&state.api.pool, &path).await {
            Ok(PublicRoute::Page(page)) => {
                return match *request.method() {
                    Method::GET | Method::HEAD => page_response(&page),
                    _ => method_not_allowed(),
                };
            }
            Ok(PublicRoute::Form(form)) => {
                return form_response(&state, request, &form, next).await;
            }
            // A Desk dashboard document: the SSR shell is the answer, exactly
            // as for a plugin-declared one.
            Ok(PublicRoute::Dashboard) => return next.run(request).await,
            // A storage failure is a 404 for this request, never a 200 with an
            // empty page.
            Err(error) => {
                warn!(%error, route = %path, "public site lookup failed; answering 404");
            }
            Ok(PublicRoute::NotFound) => {}
        }
    }
    live_page_guard(request, next).await
}

/// Whether the live registry (`ferris_core::ui_extensions`, read per
/// request) declares a page at `path`. The registry is populated when each
/// plugin's `describe()` registers, so a dev hot-reload that adds a route
/// serves without a restart — the whole point of the guard over the
/// boot-time `plugin_page_paths()` snapshot. Dashboard routes cannot sit
/// on a reserved prefix (rejected at registration), so a match here never
/// shadows an API, static or desk path.
fn is_declared_page(path: &str) -> bool {
    ferris_core::ui_extensions::registered_ui()
        .iter()
        .any(|app| {
            app.dashboards
                .iter()
                .any(|dashboard| page_path_matches(&dashboard.route, path))
                || app.website.as_ref().is_some_and(|website| {
                    website
                        .routes
                        .iter()
                        .any(|route| page_path_matches(&route.path, path))
                })
        })
}

/// A declared pattern (`/blog/:slug`, `/docs/*rest`) against a request
/// path, segment by segment: `:name` consumes one segment, `*rest` the
/// remainder (it is always the last declared segment, axum's rule too), a
/// literal only itself. Empty segments collapse on both sides — the same
/// normalization [`axum_page_path`] does on the way in.
pub(super) fn page_path_matches(declared: &str, path: &str) -> bool {
    let declared: Vec<&str> = declared
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let actual: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let mut taken = 0;
    for (index, segment) in declared.iter().enumerate() {
        if segment.starts_with('*') {
            return index == declared.len() - 1;
        }
        match actual.get(taken) {
            Some(&value) if segment.starts_with(':') || *segment == value => taken += 1,
            _ => return false,
        }
    }
    taken == actual.len()
}

/// Whether a path could name a stored public route at all.
///
/// The Desk's reserved prefixes are never CMS routes (the same rule the
/// registry applies to plugin dashboards, reused here) and a path whose last
/// segment has a file extension is an asset. Answering those without a database
/// round-trip is what keeps a scanner's 404s from turning into three engine
/// queries each.
fn could_be_stored_route(path: &str) -> bool {
    ferris_core::ui_extensions::dashboard_route_problem(path).is_none()
        && !path
            .rsplit('/')
            .next()
            .is_some_and(|last| last.contains('.'))
}

/// The one answer a stored route gives to a method it does not implement.
fn method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, "GET, HEAD, POST")],
        "Method Not Allowed",
    )
        .into_response()
}

/// The stored page as a complete HTML document.
fn page_response(page: &WebPage) -> Response {
    let rendered = ferris_web::website::WebsiteRouter::new().render_page(page);
    html_response(StatusCode::OK, rendered.html)
}

/// GET renders the form; POST is the submission handler; anything else is a
/// 405 with the methods the route supports. A form is never served on a method
/// that could create a document without going through the submission gate.
async fn form_response(
    _state: &AppState,
    request: Request,
    form: &WebForm,
    next: Next,
) -> Response {
    match *request.method() {
        Method::GET | Method::HEAD => {
            let token = ferris_auth::login::csrf_from_extensions(request.extensions()).await;
            let action = request.uri().path().to_string();
            html_response(
                StatusCode::OK,
                form.render_html(&action, token.as_deref(), None),
            )
        }
        Method::POST => next.run(request).await,
        _ => method_not_allowed(),
    }
}

/// The POST handler for a published `Web Form`.
///
/// Gate order matters: route (404), rate limit (429), body (400), honeypot
/// (acknowledged without writing), CSRF (403), login requirement (403), then
/// validation (400, with the message rendered back). Every rejection is an
/// HTML response the person can read, because this endpoint is reached by a
/// plain `<form>` and never by the Desk's JSON client.
pub(super) async fn website_form_submit(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let path = normalize_route(request.uri().path());
    let Ok(PublicRoute::Form(form)) = resolve_public_route(&state.api.pool, &path).await else {
        return not_found_fallback(request).await;
    };
    // Resolve the caller and the session's CSRF proof before the request is
    // split: both live in request extensions, and `Form::from_request`
    // consumes the reconstructed request below.
    let session_csrf = ferris_auth::login::csrf_from_extensions(request.extensions()).await;
    let user = ferris_auth::middleware::resolve_principal(
        request.headers(),
        request.extensions(),
        &state.auth,
    )
    .await;
    let (parts, body) = request.into_parts();
    let headers = parts.headers.clone();
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0);
    let limiter_key = format!("website-form:{path}:{}", caller_key(peer));
    if FORM_LIMITER.is_blocked(&limiter_key) {
        return html_response(
            StatusCode::TOO_MANY_REQUESTS,
            form.render_html(
                &path,
                None,
                Some("Too many submissions; try again in a few minutes."),
            ),
        );
    }
    let Ok(Form(fields)) =
        Form::<HashMap<String, String>>::from_request(Request::from_parts(parts, body), &state)
            .await
    else {
        return html_response(
            StatusCode::BAD_REQUEST,
            form.render_html(&path, None, Some("The form data could not be read.")),
        );
    };
    // Honeypot: a bot fills the hidden field. Acknowledge without writing so it
    // learns nothing, and without counting it as a real attempt.
    if fields
        .get("_website")
        .is_some_and(|value| !value.trim().is_empty())
    {
        return html_response(StatusCode::OK, form.render_success());
    }
    if !csrf_ok(
        &state,
        &headers,
        session_csrf.as_deref(),
        fields.get("_csrf").map(String::as_str),
    )
    .await
    {
        FORM_LIMITER.record_failure(&limiter_key);
        return html_response(
            StatusCode::FORBIDDEN,
            form.render_html(
                &path,
                None,
                Some("The security token is missing or invalid. Reload the page and try again."),
            ),
        );
    }
    if !form.can_submit(user.as_ref()) {
        FORM_LIMITER.record_failure(&limiter_key);
        return html_response(
            StatusCode::FORBIDDEN,
            form.render_html(&path, None, Some("Sign in to submit this form.")),
        );
    }
    let submission = form.submission_from_fields(&fields);
    match form
        .submit(&state.api.pool, user.as_ref(), &submission)
        .await
    {
        Ok(_) => {
            FORM_LIMITER.record_success(&limiter_key);
            html_response(StatusCode::OK, form.render_success())
        }
        Err(error) => {
            FORM_LIMITER.record_failure(&limiter_key);
            let message = match &error {
                // A validation problem belongs to the caller and is safe to
                // show; a storage/permission error is not.
                Error::Validation(problem) => problem.to_string(),
                Error::Document(DocumentError::NotFound { .. }) => {
                    "The form target no longer exists.".to_string()
                }
                _ => {
                    warn!(%error, route = %path, "public form submission failed");
                    "The submission could not be saved.".to_string()
                }
            };
            html_response(
                StatusCode::BAD_REQUEST,
                form.render_html(&path, None, Some(&message)),
            )
        }
    }
}

/// Whether the submission carries a valid anti-CSRF proof.
///
/// A caller with a session cookie must present that session's token, in the
/// `X-CSRF-Token` header or the hidden `_csrf` field. A guest has no ambient
/// credential to ride, so there is nothing a forged cross-site request could
/// gain — but the browser `Origin` is still compared with `Host` when present,
/// which is the cheap check that catches a cross-origin form post.
async fn csrf_ok(
    state: &AppState,
    headers: &HeaderMap,
    session_csrf: Option<&str>,
    field_token: Option<&str>,
) -> bool {
    let has_session = cookie_value(headers, &state.auth.config.cookie_name).is_some();
    if !has_session {
        return origin_matches(headers);
    }
    let token = headers
        .get(ferris_auth::CSRF_HEADER.as_str())
        .and_then(|value| value.to_str().ok())
        .or(field_token);
    let (Some(stored), Some(token)) = (session_csrf, token) else {
        return false;
    };
    ferris_auth::crypto::constant_time_eq(stored.as_bytes(), token.as_bytes())
}

/// A named cookie from the raw `Cookie` header.
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| key.trim() == name)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Whether a browser `Origin` matches the request `Host`.
fn origin_matches(headers: &HeaderMap) -> bool {
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let Some(origin) = origin else {
        return true;
    };
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    origin
        .split("://")
        .nth(1)
        .and_then(|authority| authority.split('/').next())
        == Some(host)
}

/// The rate-limit identity of a caller: the TCP peer when the listener provides
/// one, otherwise a shared `unknown` bucket. Forwarding headers are deliberately
/// not consulted — they are caller-controlled and would make the limit
/// bypassable by rotating a header.
fn caller_key(peer: Option<SocketAddr>) -> String {
    peer.map_or_else(|| "unknown".to_string(), |addr| addr.ip().to_string())
}

/// A complete HTML response. Public pages are never cached at this layer: a
/// publish/unpublish edit must be visible on the next load, not after a browser
/// cache expires.
fn html_response(status: StatusCode, body: String) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_core::Value;

    fn stored_page(published: bool) -> Value {
        ferris_core::value!({
            "route": "/about",
            "title": "About",
            "main_section": "<p>Hello</p>",
            "content_type": "HTML",
            "published": published,
        })
    }

    /// A published stored page becomes a 200 with its content; the same row
    /// with `published` off resolves to nothing, which is what the guard turns
    /// into the ordinary 404. A draft is never a preview.
    #[test]
    fn a_published_page_is_served_and_a_draft_is_absent() {
        let page = WebPage::from_value(&stored_page(true)).expect("published page resolves");
        let response = page_response(&page);
        assert_eq!(response.status(), StatusCode::OK);
        assert!(WebPage::from_value(&stored_page(false)).is_none());
    }

    /// A missing route and an unpublished one are indistinguishable: both
    /// resolve to `NotFound`, so the caller cannot probe for drafts.
    #[test]
    fn drafts_and_unknown_routes_are_the_same_answer() {
        let known = WebPage::from_value(&stored_page(true));
        assert!(known.is_some());
        assert!(WebPage::from_value(&stored_page(false)).is_none());
        assert!(WebPage::from_value(&ferris_core::value!({ "route": "/x" })).is_none());
    }

    #[test]
    fn csrf_origin_must_match_the_host() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "example.test".parse().expect("valid host"));
        headers.insert(
            header::ORIGIN,
            "https://example.test".parse().expect("valid origin"),
        );
        assert!(origin_matches(&headers));

        headers.insert(
            header::ORIGIN,
            "https://evil.test".parse().expect("valid origin"),
        );
        assert!(!origin_matches(&headers));

        headers.remove(header::ORIGIN);
        assert!(origin_matches(&headers));
    }

    #[test]
    fn the_session_cookie_is_read_by_name() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "theme=dark; ferris_session=abc123; other=1"
                .parse()
                .expect("valid cookie header"),
        );
        assert_eq!(
            cookie_value(&headers, "ferris_session").as_deref(),
            Some("abc123")
        );
        assert!(cookie_value(&headers, "missing").is_none());
    }

    /// The database is only consulted for paths that could name a page: the
    /// reserved namespaces and asset-looking paths answer without a query, so
    /// an unknown `/api/...` or `/favicon.ico` stays as cheap as it was.
    #[test]
    fn only_page_shaped_paths_reach_the_database() {
        assert!(could_be_stored_route("/about"));
        assert!(could_be_stored_route("/blog/hello-world"));
        assert!(!could_be_stored_route("/api/unknown"));
        assert!(!could_be_stored_route("/static/desk.css"));
        assert!(!could_be_stored_route("/favicon.ico"));
        assert!(!could_be_stored_route("/"));
    }
}
