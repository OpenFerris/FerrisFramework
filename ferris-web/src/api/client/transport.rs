//! The transport layer: exactly one HTTP stack per target.
//!
//! Split out of `mod.rs` so the endpoint wrappers and the wire they ride on are
//! readable separately (`AGENTS.md` §4 — no file past 600 lines). Every verb in
//! [`super`] funnels through [`api_request_raw`], so a target carries one stack:
//! the browser always speaks `fetch`, and `reqwest` is compiled for native only.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;

use serde_json::Value;

use super::{CSRF_HEADER_NAME, capture_csrf_token, csrf_token, get_base_url, is_mutating};

/// Low-level request with the HTTP status exposed, so callers can distinguish
/// 401 (needs login) from transport failures. Shared by the session helpers.
///
/// Every verb funnels through here so each target carries exactly one HTTP
/// stack: the browser always speaks `fetch`, and `reqwest` is compiled for
/// native only.
pub async fn api_request_raw(
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<(u16, String), String> {
    // A full page load starts with no CSRF token in memory: the boot's session
    // probe and the first view request race, and the view's `POST /list` can
    // lose — which is a 403 on every reloaded list/form page until another
    // response refreshes the token. Probe once, before the first mutation. The
    // login endpoints are exempt: a guest has no session to probe yet.
    if is_mutating(method)
        && csrf_token().is_none()
        && !path.starts_with("/api/auth/login")
        && !path.starts_with("/api/auth/mfa")
    {
        let _ = Box::pin(api_request_raw("GET", "/api/auth/session", None)).await;
    }
    let url = format!("{}{}", get_base_url(), path);
    let response = {
        #[cfg(target_arch = "wasm32")]
        {
            wasm_fetch_raw(method, &url, body).await
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            reqwest_request_raw(method, &url, body).await
        }
    };
    // A 401 on anything but the auth endpoints means the session expired while
    // the Desk was open. Report it once, process-wide, so the shell can show the
    // re-login modal over the live page instead of throwing the form away with
    // a navigation. A 401 from `/api/auth/login` is a wrong password, not an
    // expiry, and the login page already owns that message.
    if let Ok((status, _)) = &response
        && *status == 401
        && !path.starts_with("/api/auth/")
    {
        crate::desk::relogin::note_unauthorized();
    }
    response
}

/// Shared client so native requests reuse one connection pool. Building a
/// `Client` per call tore down the pool every time, so no connection was ever
/// kept alive between requests.
#[cfg(not(target_arch = "wasm32"))]
fn native_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// The four verbs the API uses, without parsing the method name per request;
/// anything else still goes through `from_bytes` rather than being refused.
#[cfg(not(target_arch = "wasm32"))]
fn native_method(method: &str) -> Result<reqwest::Method, String> {
    match method {
        "GET" => Ok(reqwest::Method::GET),
        "POST" => Ok(reqwest::Method::POST),
        "PUT" => Ok(reqwest::Method::PUT),
        "DELETE" => Ok(reqwest::Method::DELETE),
        other => reqwest::Method::from_bytes(other.as_bytes()).map_err(|e| e.to_string()),
    }
}

/// Native transport; the SSR path talks in-process so this is only exercised
/// by a native build without the `ssr` feature.
#[cfg(not(target_arch = "wasm32"))]
async fn reqwest_request_raw(
    method: &str,
    url: &str,
    body: Option<&str>,
) -> Result<(u16, String), String> {
    let mut req = native_client().request(native_method(method)?, url);
    if let Some(body) = body {
        req = req
            .header("content-type", "application/json")
            .body(body.to_owned());
    }
    // Mutating requests echo the session's CSRF token; the server
    // exempts API-key callers, and this client authenticates by cookie.
    if is_mutating(method)
        && let Some(token) = csrf_token()
    {
        req = req.header(CSRF_HEADER_NAME, token);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    capture_csrf_token(
        resp.headers()
            .get(CSRF_HEADER_NAME)
            .and_then(|v| v.to_str().ok()),
    );
    let text = resp.text().await.map_err(|e| e.to_string())?;
    Ok((status, text))
}

/// Wasm transport over the browser `fetch` API. Same-origin request cookies
/// are sent by default (`same-origin` credentials), so the session cookie the
/// login endpoint sets is carried on the following `/api/...` calls.
///
/// Mutating requests echo the session's CSRF token in the
/// `x-csrf-token` header the server's `require_csrf` gate checks; every
/// response (login, session, …) refreshes the stored token from the
/// `X-CSRF-Token` header.
#[cfg(target_arch = "wasm32")]
async fn wasm_fetch_raw(
    method: &str,
    url: &str,
    body: Option<&str>,
) -> Result<(u16, String), String> {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Headers, Request, RequestInit, Response, Window};

    let window: Window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let init = RequestInit::new();
    init.set_method(method);

    // Headers are needed when there is a JSON body, or when the request is
    // mutating and must echo the session CSRF token (a body-less DELETE still
    // has to carry it).
    let needs_headers = body.is_some() || is_mutating(method);
    let req = if needs_headers {
        let headers =
            Headers::new().map_err(|e| e.as_string().unwrap_or_else(|| "headers".to_string()))?;
        if body.is_some() {
            headers
                .set("content-type", "application/json")
                .map_err(|e| e.as_string().unwrap_or_else(|| "header set".to_string()))?;
        }
        if let Some(token) = csrf_token() {
            headers
                .set(CSRF_HEADER_NAME, &token)
                .map_err(|e| e.as_string().unwrap_or_else(|| "header set".to_string()))?;
        }
        init.set_headers(&headers);
        if let Some(b) = body {
            init.set_body(&JsValue::from_str(b));
        }
        Request::new_with_str_and_init(url, &init)
            .map_err(|e| e.as_string().unwrap_or_else(|| "request".to_string()))?
    } else {
        Request::new_with_str_and_init(url, &init)
            .map_err(|e| e.as_string().unwrap_or_else(|| "request".to_string()))?
    };
    let resp = JsFuture::from(window.fetch_with_request(&req))
        .await
        .map_err(|j| j.as_string().unwrap_or_else(|| "fetch".to_string()))?
        .dyn_into::<Response>()
        .map_err(|_| "not a Response".to_string())?;
    let status = resp.status();
    // Refresh the stored CSRF token from the response. Same-origin responses
    // expose every header; cross-origin ones only the `expose_headers` the
    // server configured (the server's CORS layer exposes `x-csrf-token`).
    let csrf = resp.headers().get(CSRF_HEADER_NAME).ok().flatten();
    capture_csrf_token(csrf.as_deref());
    let text_promise = resp
        .text()
        .map_err(|e| e.as_string().unwrap_or_else(|| "body".to_string()))?;
    let text = JsFuture::from(text_promise)
        .await
        .map_err(|j| j.as_string().unwrap_or_else(|| "body read".to_string()))?
        .as_string()
        .ok_or_else(|| "non-text body".to_string())?;
    Ok((status, text))
}

/// The API's human `message` for a non-2xx body, falling back to the raw text.
///
/// The Desk used to paint `404: {"error":…,"message":"schema error: doctype
/// not found: Task"}` inside its banners: JSON is not a sentence, and the
/// status prefix is noise for the operator. The server already writes a
/// message for every error response; use it when present.
pub fn error_message(status: u16, text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| value.get("error").and_then(Value::as_str))
                .map(str::to_string)
        })
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| format!("{status}: {text}"))
}

/// JSON body of any HTTP method; errors when the status is not 2xx.
pub async fn api_json(method: &str, path: &str, body: Option<&str>) -> Result<Value, String> {
    let (status, text) = api_request_raw(method, path, body).await?;
    if (200..300).contains(&status) {
        if text.trim().is_empty() {
            Ok(Value::Null)
        } else {
            serde_json::from_str(&text).map_err(|e| e.to_string())
        }
    } else {
        Err(error_message(status, &text))
    }
}
