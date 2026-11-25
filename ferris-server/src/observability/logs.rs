//! Event-driven observability: one finished request becomes its log rows, plus
//! the explicit internal producers for the audit rows the request layer cannot
//! see (login, logout, file access).
//!
//! The request path never touches the database. It hands the finished request
//! to a process-wide bounded queue and the responsible worker batches the rows;
//! when the queue is full the record is dropped (and counted) instead of making
//! the caller wait. The same worker owns the daily retention task, so a process
//! that logs also prunes.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use ferris_core::Value;
use ferris_db::sql::Ident;
use ferris_db::{Db, Stmt};
use ferris_document::chrono::Utc;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};

use super::{
    RequestUser, api_request_is_external, http_logging_enabled, may_log, should_log,
    trusted_client_ip, trusted_proxy_snapshot, view_target, write_named_doc, write_row,
};
use crate::router::AppState;
use crate::tls::TlsClientInfo;

/// How many finished requests may wait for the logger worker.
///
/// Bounded on purpose: an unresponsive database must cost log rows, never
/// request latency or unbounded memory. 4096 records is a few hundred KiB and
/// covers a short database hiccup at normal traffic.
const REQUEST_LOG_QUEUE_CAPACITY: usize = 4096;

/// Largest batch drained before the worker writes and returns to the channel.
const REQUEST_LOG_BATCH_SIZE: usize = 64;

/// How long the worker waits for a batch to fill once it has one record.
const REQUEST_LOG_BATCH_WINDOW: Duration = Duration::from_millis(50);

/// How often the retention task deletes expired request-log rows.
const LOG_RETENTION_INTERVAL: Duration = Duration::from_hours(24);

/// `FERRIS_LOG_RETENTION_DAYS` default; `0` disables retention.
const LOG_RETENTION_DEFAULT_DAYS: i64 = 30;

/// Row tables retention may prune. `Error Log` is operator evidence and stays
/// until a separate switch exists.
const RETENTION_DOCTYPES: &[&str] = &["Path Log", "API Request Log"];

/// Requests dropped because the queue was full, since process start.
static DROPPED_REQUESTS: AtomicU64 = AtomicU64::new(0);

/// The active worker and the channel the request path writes into.
///
/// One process-wide slot is enough and keeps a single writer per database. A
/// request from a different `Db` (a second install in the same process, a test)
/// replaces the worker; the old sender drops, its worker drains and exits, and
/// its retention task is signalled through [`Self::_retention_stop`].
struct RequestLogWorker {
    db_id: u64,
    sender: Sender<FinishedRequest>,
    _retention_stop: oneshot::Sender<()>,
}

static REQUEST_LOG_WORKER: OnceLock<Mutex<Option<RequestLogWorker>>> = OnceLock::new();

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
/// same `AuthState` the gates use (a session cookie or API key) so the row names
/// the actual caller. The recorded address is the TCP peer, and
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
    // extension keeps the logger from repeating a session lookup. Otherwise the
    // principal is resolved **before** `next.run` consumes the request: the
    // session and `axum-login` live in request extensions, not in a header the
    // logger could parse afterwards. Only a request that can still produce a row
    // pays for the lookup.
    let cached_user = request
        .extensions()
        .get::<RequestUser>()
        .map(|user| user.0.clone());
    let user = if !logging_enabled {
        None
    } else if let Some(user) = cached_user {
        Some(user)
    } else {
        ferris_auth::middleware::resolve_principal(
            request.headers(),
            request.extensions(),
            &state.auth,
        )
        .await
        .map(|user| user.id)
    };

    let response = next.run(request).await;
    let status = response.status().as_u16();

    if !logging_enabled || !should_log(&path, status) {
        return response;
    }

    let duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
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

    enqueue_request(&state.api.pool, finished);

    response
}

/// Hand one finished request to the logger worker without waiting.
///
/// `try_send` is the whole contract: never block, never await. A full queue
/// drops the record and bumps [`DROPPED_REQUESTS`]; a dead worker (a panic) is
/// noticed as a closed channel and recreated lazily by the next request.
///
/// The lock is held across `try_send` on purpose: the restart decision and the
/// channel write must see the same worker, and neither call awaits. Releasing
/// the lock between them would let two requests replace a worker under each
/// other and drop a live channel.
#[allow(clippy::significant_drop_tightening)]
fn enqueue_request(pool: &Db, request: FinishedRequest) {
    let state = REQUEST_LOG_WORKER.get_or_init(|| Mutex::new(None));
    let mut state = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut request = request;
    let mut restarted = false;
    loop {
        if state
            .as_ref()
            .is_none_or(|worker| worker.db_id != pool.id())
        {
            *state = Some(start_worker(pool));
        }
        let Some(worker) = state.as_ref() else {
            continue;
        };
        match try_enqueue(&worker.sender, request) {
            EnqueueOutcome::Queued => return,
            EnqueueOutcome::Dropped => {
                let dropped = DROPPED_REQUESTS.fetch_add(1, Ordering::Relaxed) + 1;
                debug!(dropped, "request log queue is full; row dropped");
                return;
            }
            EnqueueOutcome::Closed(returned) => {
                *state = None;
                request = returned;
                if restarted {
                    // The freshly spawned worker died immediately (or cannot be
                    // started). Do not spin on a broken runtime: drop instead.
                    let dropped = DROPPED_REQUESTS.fetch_add(1, Ordering::Relaxed) + 1;
                    warn!(dropped, "request log worker stopped again; row dropped");
                    return;
                }
                restarted = true;
                debug!("request log worker stopped; restarting it");
            }
        }
    }
}

/// What `try_send` did with one record.
#[derive(Debug)]
enum EnqueueOutcome {
    Queued,
    Dropped,
    /// The receiver is gone; the record comes back so a restart can retry it.
    Closed(FinishedRequest),
}

fn try_enqueue(sender: &Sender<FinishedRequest>, request: FinishedRequest) -> EnqueueOutcome {
    match sender.try_send(request) {
        Ok(()) => EnqueueOutcome::Queued,
        Err(mpsc::error::TrySendError::Full(_)) => EnqueueOutcome::Dropped,
        Err(mpsc::error::TrySendError::Closed(request)) => EnqueueOutcome::Closed(request),
    }
}

/// Create the channel, the batching worker and the retention task for `pool`.
fn start_worker(pool: &Db) -> RequestLogWorker {
    let (sender, receiver) = mpsc::channel(REQUEST_LOG_QUEUE_CAPACITY);
    tokio::spawn(run_request_log_worker(receiver, pool.clone()));

    let (retention_stop, retention_stopped) = oneshot::channel();
    tokio::spawn(run_retention(retention_stopped, pool.clone()));

    RequestLogWorker {
        db_id: pool.id(),
        sender,
        _retention_stop: retention_stop,
    }
}

/// Consume finished requests, batching up to [`REQUEST_LOG_BATCH_SIZE`] records
/// or [`REQUEST_LOG_BATCH_WINDOW`] before writing them.
///
/// Order within a batch is irrelevant; the rows carry their own timestamps.
/// Writes stay sequential so a backlog cannot open dozens of connections at
/// once and starve the request path's own queries.
async fn run_request_log_worker(mut receiver: Receiver<FinishedRequest>, pool: Db) {
    let mut batch = Vec::with_capacity(REQUEST_LOG_BATCH_SIZE);
    loop {
        let Some(first) = receiver.recv().await else {
            return;
        };
        batch.push(first);
        let deadline = tokio::time::Instant::now() + REQUEST_LOG_BATCH_WINDOW;
        while batch.len() < REQUEST_LOG_BATCH_SIZE {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, receiver.recv()).await {
                Ok(Some(request)) => batch.push(request),
                Ok(None) | Err(_) => break,
            }
        }
        // Batch order is irrelevant (the rows carry their own timestamps), and
        // popping keeps the allocation for the next batch.
        while let Some(request) = batch.pop() {
            write_request_rows(&pool, request).await;
        }
    }
}

/// Delete expired request-log rows once per day until the worker is replaced.
async fn run_retention(mut stop: oneshot::Receiver<()>, pool: Db) {
    let mut ticker = tokio::time::interval(LOG_RETENTION_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => match retention_once(&pool).await {
                Ok(deleted) => info!(deleted, "request log retention finished"),
                Err(error) => warn!(%error, "request log retention failed"),
            },
            _ = &mut stop => return,
        }
    }
}

/// Parse `FERRIS_LOG_RETENTION_DAYS`; `0` (or a negative value) disables it.
///
/// An unparsable value falls back to the default instead of silently keeping
/// rows forever.
fn retention_days_from(raw: Option<&str>) -> Option<i64> {
    match raw.map(str::trim) {
        None | Some("") => Some(LOG_RETENTION_DEFAULT_DAYS),
        Some(value) => match value.parse::<i64>() {
            Ok(days) if days > 0 => Some(days),
            Ok(_) => None,
            Err(_) => {
                warn!(
                    value = value,
                    "invalid FERRIS_LOG_RETENTION_DAYS; using the default"
                );
                Some(LOG_RETENTION_DEFAULT_DAYS)
            }
        },
    }
}

fn retention_days() -> Option<i64> {
    let raw = std::env::var("FERRIS_LOG_RETENTION_DAYS").ok();
    retention_days_from(raw.as_deref())
}

/// Run retention with the configured window.
async fn retention_once(pool: &Db) -> ferris_core::Result<u64> {
    let Some(days) = retention_days() else {
        return Ok(0);
    };
    let cutoff = Utc::now()
        .timestamp()
        .saturating_sub(days.saturating_mul(86_400));
    purge_older_than(pool, cutoff).await
}

/// Delete rows whose `creation` (Unix seconds) is older than `cutoff`.
///
/// The `creation` column is duplicated out of the JSON body by the engine, so
/// this needs no JSON extraction and works identically on all three dialects.
pub(super) async fn purge_older_than(pool: &Db, cutoff: i64) -> ferris_core::Result<u64> {
    let mut deleted = 0_u64;
    for doctype in RETENTION_DOCTYPES {
        deleted += purge_doctype(pool, doctype, cutoff).await?;
    }
    Ok(deleted)
}

async fn purge_doctype(pool: &Db, doctype: &str, cutoff: i64) -> ferris_core::Result<u64> {
    let table = ferris_schema::migration::document_table(doctype)?;
    // Both tables are created lazily by the engine, not at boot: an install
    // that never saw an external API call has no `tabAPIRequestLog`. No table
    // means no rows to prune, not a retention failure repeated every day.
    if !pool.table_exists(&table).await? {
        return Ok(0);
    }
    let quoted = Ident::new(&table)?.quote(pool.dialect());
    let affected = pool
        .execute(&Stmt::new(
            format!("DELETE FROM {quoted} WHERE creation < ?"),
            vec![Value::from(cutoff)],
        ))
        .await?;
    Ok(affected.rows())
}

/// Everything one request captured for its log rows, after the decision to log.
#[derive(Debug)]
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
async fn write_request_rows(pool: &Db, request: FinishedRequest) {
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

#[cfg(test)]
mod queue_tests {
    use super::{EnqueueOutcome, FinishedRequest, REQUEST_LOG_QUEUE_CAPACITY, mpsc, try_enqueue};

    fn finished(path: &str) -> FinishedRequest {
        FinishedRequest {
            method: "GET".to_string(),
            path: path.to_string(),
            query: String::new(),
            status: 200,
            duration_ms: 1,
            ip: String::new(),
            user: None,
            external_api: false,
        }
    }

    /// The request path uses `try_send`, so a full queue must report a drop —
    /// never block the caller and never grow past the capacity.
    #[tokio::test]
    async fn a_full_queue_drops_the_record() {
        let (sender, _receiver) = mpsc::channel(REQUEST_LOG_QUEUE_CAPACITY);
        for index in 0..REQUEST_LOG_QUEUE_CAPACITY {
            assert!(
                matches!(
                    try_enqueue(&sender, finished(&format!("/queued/{index}"))),
                    EnqueueOutcome::Queued
                ),
                "record {index} within capacity must be queued"
            );
        }
        assert!(
            matches!(
                try_enqueue(&sender, finished("/overflow")),
                EnqueueOutcome::Dropped
            ),
            "the record past capacity is dropped, not queued or awaited"
        );
    }

    /// A dead worker (panic) leaves the sender closed; the record comes back so
    /// `enqueue_request` can restart the worker and retry it.
    #[tokio::test]
    async fn a_closed_queue_returns_the_record_for_a_retry() {
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        match try_enqueue(&sender, finished("/retry")) {
            EnqueueOutcome::Closed(request) => assert_eq!(request.path, "/retry"),
            other => panic!("expected a closed channel, got {other:?}"),
        }
    }

    #[test]
    fn retention_days_parse_disables_on_zero_and_defaults_on_garbage() {
        assert_eq!(super::retention_days_from(None), Some(30));
        assert_eq!(super::retention_days_from(Some(" 7 ")), Some(7));
        assert_eq!(super::retention_days_from(Some("0")), None);
        assert_eq!(super::retention_days_from(Some("-3")), None);
        assert_eq!(super::retention_days_from(Some("forever")), Some(30));
    }
}
