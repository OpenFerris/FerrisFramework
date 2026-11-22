//! Security response headers and the per-response CSP nonce.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::Response,
};

/// Request extension carrying the per-response CSP nonce minted in
/// [`security_headers_with_policy`]. The SSR render reads it back in
/// `provide_desk_context` to stamp every inline `<script>` with the same value
/// the header advertises.
#[derive(Clone, Debug)]
pub(super) struct CspNonce(pub(super) Arc<str>);

/// What the process knows about its own transport, fixed at boot.
///
/// The header set must not lie about the deployment: `Strict-Transport-Security`
/// on a plain-HTTP listener is noise (and dangerous for sibling subdomains), so
/// the fused router passes whether `[server] tls` is configured. Reverse-proxy
/// TLS is deliberately not inferred from a header — an unauthenticated
/// `X-Forwarded-Proto` would let a client talk the server into a one-year HSTS
/// pin.
#[derive(Clone, Copy, Debug)]
pub(super) struct SecurityPolicy {
    tls: bool,
}

impl SecurityPolicy {
    pub(super) const fn new(tls: bool) -> Self {
        Self { tls }
    }
}

/// Content-Security-Policy built per response.
///
/// Strict default-deny. `script-src` keeps `'self'` for the hydration bundle
/// and adds a per-response `'nonce-…'` for the inline scripts the `Desk` emits
/// (Leptos' streaming bootstrap, the `leptos_meta` globals and the component
/// tooling); `'wasm-unsafe-eval'` lets the same-origin wasm bundle instantiate.
/// Styles stay `'unsafe-inline'` (the SSR output styles inline),
/// storage/report embeds use `data:`/`blob:`, and `img-src` additionally names
/// the one remote host the `Geolocation` widget loads map tiles from — a
/// concrete host (`https://tile.openstreetmap.org`), never a bare `https:`
/// wildcard, so no other origin can be pulled in as an image. `connect-src`
/// stays `'self'`: socket.io and the SSE bus are same-origin, so a bare `ws:`
/// would only widen the exfiltration surface for an injected script. The
/// realtime client upgrades to a same-origin `WebSocket`, and
/// `frame-ancestors 'self'` (plus `X-Frame-Options: SAMEORIGIN`) allows
/// same-origin embedding while refusing cross-origin clickjacking.
fn content_security_policy(nonce: &str) -> String {
    format!(
        "default-src 'self'; \
         script-src 'self' 'nonce-{nonce}' 'wasm-unsafe-eval' blob:; \
         worker-src 'self' blob:; \
         child-src 'self' blob:; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob: https://tile.openstreetmap.org; \
         font-src 'self' data:; \
         connect-src 'self'; \
         object-src 'none'; \
         base-uri 'self'; \
         form-action 'self'; \
         frame-ancestors 'self'"
    )
}

/// Whether a successful response on `path` must never be stored by a shared
/// cache.
///
/// `/api/*` answers are per-caller: a proxy that caches a JSON list for user A
/// and replays it to user B is a tenant leak with no exploit beyond a cache
/// hit. The streaming endpoints are exempt because they are long-lived
/// responses whose framing must not be rewritten; they carry their own
/// no-buffering semantics. `/events` and `/socket.io` are already outside
/// `/api`, and are listed here so the intent survives a future prefix move.
fn must_not_be_cached(path: &str) -> bool {
    const STREAMING: &[&str] = &["/api/notifications/stream", "/events", "/socket.io"];
    let streaming = STREAMING.iter().any(|stream| {
        path.strip_prefix(stream)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    });
    if streaming {
        return false;
    }
    path == "/api" || path.starts_with("/api/")
}

/// Security headers applied to every HTTP response.
///
/// The router previously sent no security headers at all, so an XSS that made
/// it into the Desk ran with full DOM and session access. CSP bounds what an
/// attacker-controlled script can load/phone home, `X-Frame-Options` +
/// `frame-ancestors` block clickjacking, `nosniff` stops MIME confusion, and
/// HSTS forces HTTPS for the whole origin — but only when this process really
/// terminates TLS (`SecurityPolicy::tls`).
///
/// The CSP carries a fresh nonce minted here and smuggled to the render on the
/// request: the render cannot write response headers, and this layer cannot see
/// the nonce `leptos_axum` generates internally, so the framework mints its own
/// and `provide_desk_context` overrides Leptos' with it. Without that, the
/// strict `script-src` would block Leptos' own inline bootstrap and nothing in
/// the Desk would ever hydrate.
async fn apply_security_headers(
    mut request: Request,
    next: Next,
    policy: SecurityPolicy,
) -> Response {
    let path = request.uri().path().to_owned();
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    request
        .extensions_mut()
        .insert(CspNonce(nonce.clone().into()));

    let mut response = next.run(request).await;
    // The policy is static text plus a hex nonce, so this cannot fail in
    // practice; handle it instead of panicking a worker one refactor away
    // (SRV12).
    let csp = match HeaderValue::from_str(&content_security_policy(&nonce)) {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(%error, "generated Content-Security-Policy is not a valid header");
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            return response;
        }
    };
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, csp);
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("SAMEORIGIN"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    // HSTS is a promise about the transport, so it is emitted only when the
    // process itself terminates TLS. `includeSubDomains` is part of the
    // framework's default pin; an operator with sibling subdomains on plain
    // HTTP should front the server with a proxy that owns this header.
    if policy.tls {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }
    if must_not_be_cached(&path) {
        // Respect a route that deliberately set its own caching policy.
        if !headers.contains_key(header::CACHE_CONTROL) {
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, no-store"),
            );
        }
        // The response body depends on the caller's cookie or bearer token.
        headers.append(
            header::VARY,
            HeaderValue::from_static("Cookie, Authorization"),
        );
    }
    headers.insert(header::X_XSS_PROTECTION, HeaderValue::from_static("0"));
    response
}

/// The fused-router entry point: applies the header set with the boot-time
/// transport policy supplied by `build_router`.
pub(super) async fn security_headers_with_policy(
    State(policy): State<SecurityPolicy>,
    request: Request,
    next: Next,
) -> Response {
    apply_security_headers(request, next, policy).await
}

/// Test-only convenience wrapper that assumes TLS termination.
///
/// Production always mounts [`security_headers_with_policy`] with the real
/// `[server] tls` state; keeping this out of non-test builds means no code path
/// can emit HSTS without knowing whether the listener speaks TLS.
#[cfg(test)]
pub(super) async fn security_headers(request: Request, next: Next) -> Response {
    apply_security_headers(request, next, SecurityPolicy::new(true)).await
}

#[cfg(test)]
mod tests {
    use axum::{Router, body::Body, http::header, middleware::from_fn_with_state, routing::get};
    use tower::ServiceExt;

    use super::{SecurityPolicy, security_headers_with_policy};

    fn app(policy: SecurityPolicy, path: &'static str) -> Router {
        Router::new()
            .route(path, get(|| async { "ok" }))
            .layer(from_fn_with_state(policy, security_headers_with_policy))
    }

    async fn headers_for(policy: SecurityPolicy, path: &'static str) -> axum::http::HeaderMap {
        let response = app(policy, path)
            .oneshot(
                axum::extract::Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("the request builds"),
            )
            .await
            .expect("the router answers");
        response.headers().clone()
    }

    /// SRV-004: the WebSocket transport is same-origin, so `connect-src` must
    /// not authorize a collector on any other host.
    #[tokio::test]
    async fn connect_src_is_self_only() {
        let headers = headers_for(SecurityPolicy::new(false), "/desk").await;
        let csp = headers
            .get(header::CONTENT_SECURITY_POLICY)
            .expect("CSP present")
            .to_str()
            .expect("ascii");
        assert!(csp.contains("connect-src 'self';"), "got: {csp}");
        assert!(
            !csp.contains("ws:"),
            "a bare ws: source is forbidden: {csp}"
        );
        assert!(
            !csp.contains("wss:"),
            "a bare wss: source is forbidden: {csp}"
        );
    }

    /// SRV-005: HSTS is a statement about the listener, so it is absent when
    /// the process does not terminate TLS.
    #[tokio::test]
    async fn hsts_is_only_emitted_when_tls_is_configured() {
        let plain = headers_for(SecurityPolicy::new(false), "/desk").await;
        assert!(
            plain.get(header::STRICT_TRANSPORT_SECURITY).is_none(),
            "plain HTTP must not pin HTTPS"
        );

        let tls = headers_for(SecurityPolicy::new(true), "/desk").await;
        assert_eq!(
            tls.get(header::STRICT_TRANSPORT_SECURITY)
                .expect("HSTS on a TLS listener"),
            "max-age=31536000; includeSubDomains"
        );
    }

    /// SRV-006: a successful API read is per-caller and must not be stored by a
    /// shared cache. The streaming endpoint keeps its own semantics.
    #[tokio::test]
    async fn api_reads_are_private_and_vary_on_the_credential() {
        let headers = headers_for(SecurityPolicy::new(false), "/api/resource/ToDo").await;
        assert_eq!(
            headers.get(header::CACHE_CONTROL).expect("cache-control"),
            "private, no-store"
        );
        let vary = headers.get(header::VARY).expect("vary").to_str().unwrap();
        assert!(vary.contains("Cookie"), "got: {vary}");
        assert!(vary.contains("Authorization"), "got: {vary}");

        let stream = headers_for(SecurityPolicy::new(false), "/api/notifications/stream").await;
        assert!(
            stream.get(header::CACHE_CONTROL).is_none(),
            "the SSE endpoint must not be rewritten"
        );

        let desk = headers_for(SecurityPolicy::new(false), "/desk").await;
        assert!(
            desk.get(header::CACHE_CONTROL).is_none(),
            "non-API responses keep their own caching policy"
        );
    }
}
