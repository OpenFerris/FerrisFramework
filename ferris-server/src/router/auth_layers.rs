//! Authentication middleware in front of the API routes.

use axum::{
    extract::{FromRequestParts, Request, State},
    middleware::Next,
    response::Response,
};
use ferris_auth::AuthState;
use ferris_auth::middleware::{OptionalUser, require_auth};
use ferris_core::ClientIdentity;

/// Runs an authenticated request with the resolved user scoped as the ambient
/// engine user so `DocStore`/typed CRUD authorises as the actual caller. This
/// must sit inside `require_auth_or_device` (it assumes authentication has
/// already been decided) and tolerates a caller with no session user — a
/// certificate-authenticated device — so the engine scopes no user and denies.
pub(super) async fn bind_ambient_user(
    State(auth): State<AuthState>,
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    let (mut parts, body) = request.into_parts();
    // Resolve the user optionally: `require_auth_or_device` (outer) already
    // enforced authentication, and a caller that proved itself with a verified
    // client certificate has no session user. The engine stays scoped as the
    // caller when there is one and denies as it would for an anonymous write
    // when there is not.
    let user = OptionalUser::from_request_parts(&mut parts, &auth)
        .await
        .map(|extractor| extractor.0)
        // `OptionalUser::Rejection` is `Infallible`; spelling the arm out keeps
        // the request path free of `expect` (SRV12).
        .unwrap_or_else(|never| match never {});
    let request = Request::from_parts(parts, body);
    Ok(ferris_document::with_user(user, async move { next.run(request).await }).await)
}

/// Authenticate an API request, additionally accepting a **verified client
/// certificate** as the credential for RPC method calls (`/api/method/*`).
///
/// H1 (plan Fase 2): a device that only holds a CA-issued certificate has no
/// session and no API key, so the plain `require_auth` gate answered 401
/// and the plugin was reachable only through `/plugins/{name}/*`. The
/// certificate is bound by `bind_client_identity`, which must run **before**
/// this layer (it is outermost on the API router). Device access is deliberately
/// limited to the method path: on `/api/resource/*` a certificate never
/// substitutes for a role, so those routes keep the plain session gate.
pub(super) async fn require_auth_or_device(
    State(state): State<AuthState>,
    request: Request,
    next: Next,
) -> Result<Response, Response> {
    if request.uri().path().starts_with("/api/method/")
        && request.extensions().get::<ClientIdentity>().is_some()
    {
        return Ok(next.run(request).await);
    }
    require_auth(State(state), request, next).await
}
