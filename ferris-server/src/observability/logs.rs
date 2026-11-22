//! Event-driven observability: one finished request becomes its log rows, plus
//! the explicit internal producers for the audit rows the request layer cannot
//! see (login, logout, file access).

use std::net::{IpAddr, SocketAddr};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::Response;

use super::{
    RequestUser, api_request_is_external, http_logging_enabled, may_log, should_log,
    trusted_client_ip, trusted_proxy_snapshot, view_target, write_named_doc, write_row,
};
use crate::router::AppState;
use crate::tls::TlsClientInfo;

/// The TCP peer, when the listener inserted `ConnectInfo` for it.
///
/// The TLS listener publishes [`TlsClientInfo`]; the plain-HTTP listener would
/// publish a `SocketAddr`. Both are handled so enabling connect info on the
/// plain listener needs no change here.
fn request_peer(request: &Request) -> Option<IpAddr> {
    if let Some(ConnectInfo(info)) = request.extensions().get::<ConnectInfo<TlsClientInfo>>() {
        return Some(info.peer.ip());
    }
    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(peer)| peer.ip())
}

/// Axum layer: turn one finished request into its System log rows.
///
/// The request is only inspected, never consumed; the user is resolved from the
/// same headers the auth middleware uses (a session cookie or bearer token) so
/// the row names the actual caller. The recorded address is the TCP peer, and
/// `X-Forwarded-For`/`X-Real-IP` are read only when that peer is listed in
/// `[server] trusted_proxies` (SEC-08).
pub async fn record_request(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let method = request.method().as_str().to_string();
    let path = request.uri().path().to_string();
    let query = request.uri().query().unwrap_or_default().to_string();
    let ip = trusted_client_ip(
        request.headers(),
        request_peer(&request),
        &trusted_proxy_snapshot(),
    );
    // Decide from the incoming headers, before `next.run` consumes the request.
    // Only `API Request Log` uses this; `Path Log` remains the complete request
    // history, including the Desk's own calls. Without `external_url` every API
    // call is recorded rather than trusting the request's own `Host`.
    let external_api =
        api_request_is_external(request.headers(), state.api_log_base_origin.as_deref());
    // PERF-01: resolving the principal costs a session/token lookup, and most
    // requests (static assets, plugin assets, a disabled logger) never produce
    // a row. The path half of `should_log` is status-independent, so it runs
    // before any credential work; the status half still runs after `next`.
    let logging_enabled = http_logging_enabled() && may_log(&path);
    // An upstream layer may have resolved the caller already. Reusing that
    // extension keeps the logger from repeating a session lookup; when it is
    // absent the headers are cloned only for a request that can still be
    // logged, so `next.run` can consume the original request.
    let cached_user = request
        .extensions()
        .get::<RequestUser>()
        .map(|user| user.0.clone());
    let headers = (logging_enabled && cached_user.is_none()).then(|| request.headers().clone());

    let response = next.run(request).await;
    let status = response.status().as_u16();

    if !logging_enabled || !should_log(&path, status) {
        return response;
    }

    let user = match cached_user {
        Some(user) => Some(user),
        None => match headers {
            Some(headers) => ferris_auth::middleware::resolve_principal(&headers, &state.auth)
                .await
                .map(|user| user.id),
            None => None,
        },
    };

    let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    let pool = state.api.pool.clone();
    let finished = FinishedRequest {
        method,
        path,
        query,
        status,
        duration_ms,
        ip,
        user,
        external_api,
    };

    tokio::spawn(async move { write_request_rows(&pool, finished).await });

    response
}

/// Everything one request captured for its log rows, after the decision to log.
struct FinishedRequest {
    method: String,
    path: String,
    query: String,
    status: u16,
    duration_ms: i64,
    ip: String,
    user: Option<String>,
    external_api: bool,
}

/// Write the `Path Log`/`API Request Log`/`Error Log`/`View Log` rows of one
/// finished request.
///
/// Split out of [`record_request`] so the middleware stays a decision layer and
/// its length does not grow with each new row the System section gains.
async fn write_request_rows(pool: &ferris_db::Db, request: FinishedRequest) {
    let FinishedRequest {
        method,
        path,
        query,
        status,
        duration_ms,
        ip,
        user,
        external_api,
    } = request;
    let user = user.as_deref();
    write_row(
        pool,
        "Path Log",
        serde_json::json!({
            "method": method,
            "path": path,
            "status_code": status,
            "duration_ms": duration_ms,
            "user": user,
            "ip_address": ip,
        }),
    )
    .await;

    if !path.starts_with("/api/") {
        return;
    }
    // The Desk's own same-origin traffic is already covered by `Path Log`;
    // `API Request Log` is for calls from outside the normal browser flow
    // (devices, scripts, other origins).
    if external_api {
        let url = if query.is_empty() {
            path.clone()
        } else {
            format!("{path}?{query}")
        };
        write_row(
            pool,
            "API Request Log",
            serde_json::json!({
                "method": method,
                "url": url,
                "status_code": status,
                "duration_ms": duration_ms,
                "user": user,
                "ip_address": ip,
            }),
        )
        .await;
    }

    if status >= 500 {
        write_row(
            pool,
            "Error Log",
            serde_json::json!({
                "error_type": format!("HTTP {status}"),
                "error": format!("{method} {path} answered {status}"),
                "traceback": "",
            }),
        )
        .await;
    }

    if method == "GET"
        && status == 200
        && let Some((doctype, name)) = view_target(&path)
    {
        write_row(
            pool,
            "View Log",
            serde_json::json!({
                "user": user,
                "reference_doctype": doctype,
                "reference_name": name,
            }),
        )
        .await;
    }
}

/// Stable per-event document name: identical arguments in the same second map
/// to the same row, so a retried producer is idempotent and a genuinely new
/// event (a later login) still gets its own row.
fn event_name(prefix: &str, kind: &str, user: &str, ip: &str, detail: &str) -> String {
    let at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    // FNV-1a: no dependency, deterministic across processes, and only used to
    // build a name — not as a security primitive.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in format!("{kind}\u{1f}{user}\u{1f}{ip}\u{1f}{detail}\u{1f}{at}").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{prefix}-{hash:016x}")
}

/// Record one `Activity Log` event (`login`, `logout`, …) best-effort.
///
/// This is the only supported writer for the append-only `Activity Log`: the
/// declaration grants `read` alone, and the generic `update`/`delete` path is
/// blocked by the append-only guard. A logging failure is reported at `debug`
/// and never fails the authentication operation it describes.
///
/// Call sites (outside this agent's file set; they must call this after the
/// operation succeeded):
/// * `ferris-api/src/session.rs` — `login` after `record_success` (line ~207,
///   just before `open_session`), with the resolved `user.id` and `ip`.
/// * `ferris-api/src/session.rs` — `logout` after the session is deleted
///   (line ~281), with the session's `user_id` and the request IP.
pub async fn record_activity(pool: &ferris_db::Db, kind: &str, user: &str, ip: &str, detail: &str) {
    let data = serde_json::json!({
        "name": event_name("ACT", kind, user, ip, detail),
        "user": user,
        "subject": detail,
        "operation": kind,
        "status": "Success",
        "ip_address": ip,
    });
    write_audit(pool, "Activity Log", data).await;
}

/// Record one `Access Log` row for a file read/export, best-effort.
///
/// Call sites (outside this agent's file set): `ferris-files/src/http.rs`,
/// `download` (line ~216) and `download_signed_file` (line ~479), after the
/// bytes were authorized and read, with the authenticated user.
pub async fn record_access(pool: &ferris_db::Db, user: &str, file_type: &str, reference: &str) {
    let data = serde_json::json!({
        "name": event_name("ACC", "file_access", user, "", reference),
        "user": user,
        "export_from": "File",
        "reference_document": reference,
        "file_type": file_type,
    });
    write_audit(pool, "Access Log", data).await;
}

/// Insert an audit row, treating a duplicate name as success (idempotency) and
/// every other failure as a dropped log line.
async fn write_audit(pool: &ferris_db::Db, doctype: &str, data: serde_json::Value) {
    use ferris_core::{DatabaseError, DocumentError, Error};
    match write_named_doc(pool, doctype, data).await {
        Ok(_)
        | Err(
            Error::Document(DocumentError::AlreadyExists { .. })
            | Error::Database(DatabaseError::Constraint(_)),
        ) => {}
        Err(error) => tracing::debug!(doctype, %error, "audit log write failed"),
    }
}
