//! Producers for the `System` observability `DocType` rows (SRV7).
//!
//! `doctypes.rs` declares eleven system reports and logs, and `workspaces.rs`
//! links them into the System section. Declaring them was not enough: an
//! operator opening System → Logs got permanently empty tables with no hint
//! that the feature was a placeholder. This module is the write side.
//!
//! Two kinds of producer meet here, each in its own fragment:
//!
//! * **Event-driven** (`logs.rs`) — [`record_request`] is an outermost axum
//!   layer that turns one finished request into a `Path Log` row, an
//!   `API Request Log` row for `/api/*`, an `Error Log` row for a 5xx, and a
//!   `View Log` row when a document was opened. The layer is deliberately
//!   outside `require_auth`, so it sees the final status (including timeout/404)
//!   before the body leaves.
//! * **State-driven** (`mirrors.rs`) — [`spawn`] periodically projects the
//!   queue, scheduler and prepared-report stores onto their System mirrors, and
//!   writes the health and storage snapshots. The underlying store keeps being
//!   the source of truth; the report is the read model the Desk renders.
//!
//! Writes are fire-and-forget: a logging failure must never fail the request it
//! describes, so it is reported at `debug` and dropped. They go through the
//! dynamic document engine (`EngineContext::allow_anonymous`, owner
//! `Administrator`) so validation, naming and lifecycle stay in one path instead
//! of a second hand-written `INSERT` per table.
//
// `Error` is the crate-wide error type; boxing it just to shrink the lint's
// measured `Err` size would leak into every caller. Same rationale as
// `ferris-api` and `ferris-integrations`.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{OnceLock, RwLock};

use ferris_core::Value;
use ferris_core::hooks::{HookPoint, HookRegistry};
use ferris_db::Db;
use ferris_document::engine::{DocumentEngine, EngineContext};
use tracing::debug;

mod logs;
mod mirrors;

pub use logs::{record_access, record_activity, record_request};
pub use mirrors::spawn;

/// Owner of every framework-written row, matching the convention the rest of
/// the install uses for system documents.
const SYSTEM_USER: &str = "Administrator";

/// How often the state-driven snapshot runs. Slow enough that the mirrors do
/// not dominate the database, fast enough that a worker heartbeat is never
/// stale for long.
const SNAPSHOT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

/// The stores the state-driven mirrors may read.
///
/// A struct instead of positional `Option`s because which stores exist is a
/// **compile-time** decision (`jobs`, `scheduler`): a field that is absent from
/// the build is absent from the struct, so the snapshot cannot be handed a
/// store whose feature is off and the call sites cannot drift.
#[derive(Default)]
pub struct Sources {
    /// The shared default queue: `RQ Job` and `RQ Worker` mirrors.
    #[cfg(feature = "jobs")]
    pub queue: Option<std::sync::Arc<ferris_jobs::JobQueue>>,
    /// The running scheduler: `Scheduled Job Type` / `Scheduled Job Log`.
    #[cfg(feature = "scheduler")]
    pub scheduler: Option<std::sync::Arc<ferris_scheduler::Scheduler>>,
}

impl Sources {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Bookkeeping for the state-driven mirrors.
#[derive(Default)]
struct Mirrors {
    /// `"<doctype>:<logical key>"` → document name.
    names: HashMap<String, String>,
    /// `Scheduled Job Type` id → `run_count` observed on the previous tick; a
    /// change is what mints a `Scheduled Job Log` row.
    #[cfg(feature = "scheduler")]
    schedule_runs: HashMap<String, u64>,
}

/// Engine context for a framework write: anonymous at the permission layer (the
/// host is the caller), never re-entering plugin `doctype-event` handlers.
fn system_ctx(doctype: &str) -> EngineContext<'_> {
    let mut ctx = EngineContext::new(doctype, "");
    ctx.allow_anonymous = true;
    ctx.suppress_events = true;
    ctx
}

/// `DocTypes` that record evidence and must never be mutated or deleted through
/// the generic document path.
///
/// The declarations grant only `read`, but that is a metadata policy: the core
/// permission engine short-circuits `System Manager`, and the REST delete/update
/// routes go through the same engine. The guard below is the enforcement point
/// that closes that gap (SEC-07/08/09/AUTH-17).
pub const AUDIT_DOCTYPES: &[&str] = &[
    "Path Log",
    "API Request Log",
    "View Log",
    "Error Log",
    "Activity Log",
    "Access Log",
];

/// Tag that makes re-installing the guard replace the previous registration
/// instead of stacking a second copy on every boot or hot reload.
const APPEND_ONLY_TAG: &str = "ferris.audit-append-only";

/// Reject `update` and `delete` on every [`AUDIT_DOCTYPES`] row.
///
/// Registered from [`crate::doctypes::register_doctypes`], i.e. on every boot
/// and in every test that installs the system catalog. Inserts stay open to the
/// internal producers ([`record_activity`], [`record_access`] and the request
/// logger), which write as `allow_anonymous` and never run `BeforeSave`.
pub fn install_append_only_guard() {
    for doctype in AUDIT_DOCTYPES {
        for (point, action) in [
            (HookPoint::BeforeSave, "update"),
            (HookPoint::BeforeDelete, "delete"),
        ] {
            HookRegistry::replace_global(
                APPEND_ONLY_TAG,
                (*doctype).to_string(),
                point,
                Box::new(
                    move |ctx: &mut ferris_core::hooks::HookContext,
                          _doc: &mut dyn ferris_core::hooks::DocumentHook| {
                        let resource = format!("{} is append-only", ctx.doctype);
                        Box::pin(async move {
                            Err(ferris_core::Error::Authz(
                                ferris_core::AuthzError::PermissionDenied {
                                    action: action.to_string(),
                                    resource,
                                },
                            ))
                        })
                    },
                ),
                0,
                None,
            );
        }
    }
}

async fn write_doc(
    pool: &Db,
    doctype: &str,
    data: serde_json::Value,
) -> ferris_core::Result<String> {
    let engine = DocumentEngine::new(pool.clone());
    let ctx = system_ctx(doctype);
    let document = engine
        .create_owned_by(&ctx, SYSTEM_USER, Value::from_json(data))
        .await?;
    Ok(document
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

/// Insert a row, reporting failure at `debug`: a log write must not fail the
/// operation it describes.
async fn write_row(pool: &Db, doctype: &str, data: serde_json::Value) {
    if let Err(error) = write_doc(pool, doctype, data).await {
        debug!(doctype, %error, "system log write failed");
    }
}

/// Insert an audit row with a caller-minted name.
///
/// Audit producers need a deterministic name so a retry is idempotent; the
/// generic engine path rejects explicit names unless the trusted Rust caller
/// opts in (DOC-16), which is exactly what this internal helper does.
async fn write_named_doc(
    pool: &Db,
    doctype: &str,
    data: serde_json::Value,
) -> ferris_core::Result<String> {
    let engine = DocumentEngine::new(pool.clone());
    let mut ctx = system_ctx(doctype);
    ctx.allow_explicit_name = true;
    let document = engine
        .create_owned_by(&ctx, SYSTEM_USER, Value::from_json(data))
        .await?;
    Ok(document
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

fn trusted_proxy_snapshot() -> Vec<TrustedProxy> {
    trusted_proxy_store()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Create on first sight, update afterwards.
async fn upsert(
    pool: &Db,
    mirrors: &mut Mirrors,
    key: String,
    doctype: &str,
    data: serde_json::Value,
) {
    if let Some(name) = mirrors.names.get(&key).cloned() {
        if name.is_empty() {
            return;
        }
        engine_update(pool, doctype, &name, data).await;
        return;
    }
    // A document whose name is stable across process restarts (`Scheduled Job
    // Type` uses the schedule name) already exists after a reboot while the
    // in-memory map is empty. Treat the resulting unique conflict as "update
    // the existing row", not as a mirror failure that would retry the insert
    // every tick and never refresh it.
    let stable_name = data
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    match write_doc(pool, doctype, data.clone()).await {
        Ok(name) => {
            mirrors.names.insert(key, name);
        }
        Err(
            error @ (ferris_core::Error::Document(ferris_core::DocumentError::AlreadyExists {
                ..
            })
            | ferris_core::Error::Database(ferris_core::DatabaseError::Constraint(_))),
        ) => {
            if let Some(name) = stable_name {
                engine_update(pool, doctype, &name, data).await;
                mirrors.names.insert(key, name);
            } else {
                debug!(doctype, %error, "system mirror write failed");
            }
        }
        Err(error) => debug!(doctype, %error, "system mirror write failed"),
    }
}

async fn engine_update(pool: &Db, doctype: &str, name: &str, data: serde_json::Value) {
    let engine = DocumentEngine::new(pool.clone());
    let ctx = system_ctx(doctype);
    if let Err(error) = engine.update(&ctx, name, Value::from_json(data)).await {
        debug!(doctype, %error, "system mirror update failed");
    }
}

/// Request extension publishing the principal a layer has already resolved.
///
/// The request logger is the outermost layer, so it cannot see an
/// `AuthenticatedUser` extractor's work. Any layer that resolves the caller
/// (today a host integration; the auth middleware itself is not in this crate)
/// can insert this and the logger reuses the identity instead of asking the
/// session store again (PERF-01).
#[derive(Clone, Debug)]
pub struct RequestUser(pub String);

/// Static asset prefixes: never worth a row, whatever the status.
const SKIP_PREFIXES: &[&str] = &[
    "/static",
    "/pkg",
    "/assets",
    "/vendor",
    "/previews",
    "/favicon",
];

/// Long-lived transports. A 200 is a stream that stays open, but the layer only
/// sees the response headers: logging it would record handler-construction time
/// as if the stream had closed after a few milliseconds. Only a rejection is
/// logged (a 403 on a foreign channel is exactly the kind of failed subscription
/// an operator needs to see).
const SKIP_SUCCESS_PREFIXES: &[&str] = &[
    "/events",
    "/socket.io",
    // The Desk's per-user notification SSE: same shape as `/events`. A 200 is
    // a long-lived stream, so logging it would record the handler-construction
    // time (tens of milliseconds) as if the stream had closed. Rejections are
    // still logged below.
    "/api/notifications/stream",
];

/// `FERRIS_HTTP_LOG=0|false` turns the request-path producers off, which is what
/// a load test or a very small install wants without recompiling.
fn http_logging_enabled() -> bool {
    std::env::var("FERRIS_HTTP_LOG").map_or(true, |value| {
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

/// True for a plugin's static assets (`/plugins/<app>/asset/...`).
///
/// The prefix check alone cannot express this: `/plugins` also carries real
/// endpoints (telemetry, callbacks) that must stay observable, while the
/// editor's CSS/JS files are pure noise. The app segment is variable, so the
/// deciding segment is the third one.
fn is_plugin_asset(path: &str) -> bool {
    let mut segments = path.trim_start_matches('/').split('/');
    matches!(segments.next(), Some("plugins"))
        && segments.next().is_some()
        && matches!(segments.next(), Some("asset" | "assets" | "static"))
}

/// Whether one finished request deserves a Path Log row.
///
/// The status is part of the decision because a rejected `/events` subscription
/// and an open one are the same path: the first must be recorded, the second
/// must not hold a task (and a row) for as long as the client stays connected.
fn should_log(path: &str, status: u16) -> bool {
    if SKIP_PREFIXES.iter().any(|prefix| path.starts_with(prefix)) {
        return false;
    }
    if status < 400
        && SKIP_SUCCESS_PREFIXES
            .iter()
            .any(|prefix| path.starts_with(prefix))
    {
        return false;
    }
    !is_plugin_asset(path)
}

/// The status-independent half of [`should_log`].
///
/// The request logger runs this before it resolves the principal, so an asset
/// request never triggers a session lookup (PERF-01). The remaining decision
/// (a successful long-lived stream) still needs the response status and is
/// applied by [`should_log`] after `next`.
fn may_log(path: &str) -> bool {
    !SKIP_PREFIXES.iter().any(|prefix| path.starts_with(prefix)) && !is_plugin_asset(path)
}

/// Whether an `/api/*` request should get an `API Request Log` row.
///
/// With `[server] external_url` configured, provenance is compared against that
/// authoritative origin. Without it the request is **always** recorded: falling
/// back to the request's own `Host` would let a caller who controls both
/// `Host` and `Origin` make its call look like Desk traffic and so choose not
/// to be audited. Recording a few extra same-origin rows is the safe failure
/// (SEC-08/DX-01).
fn api_request_is_external(headers: &axum::http::HeaderMap, base_origin: Option<&str>) -> bool {
    base_origin.is_none() || is_external_api_request(headers, base_origin)
}

/// Whether an `/api/*` request came from outside the Desk's own origin.
///
/// The Desk is a browser application: its normal API traffic is same-origin and
/// carries `Origin` on mutations and `Referer` on reads. Those rows are noise in
/// `API Request Log` (the operator already has `Path Log` for the full request
/// history). An external integration — a device, a script, another web app —
/// either sends an origin that is not the configured public origin, or sends no
/// browser provenance at all, and that is what this log is for.
///
/// The comparison is intentionally about provenance, not authentication: an
/// external caller that spoofs the Desk's `Origin` is still authenticated by its
/// own session/certificate, and this predicate only decides whether to write an
/// audit row. Missing `Origin`/`Referer` is treated as external, because a
/// curl/webhook call is exactly the case the operator wants to see.
///
/// `base_origin` comes from `[server] external_url`. The producer must call
/// [`api_request_is_external`], which treats a missing `external_url` as
/// external; the `Host` fallback below only exists for tests and explicit
/// callers that accept it (SEC-08).
fn is_external_api_request(headers: &axum::http::HeaderMap, base_origin: Option<&str>) -> bool {
    let request_host = header_str(headers, axum::http::header::HOST);
    let provenance = header_str(headers, axum::http::header::ORIGIN)
        .or_else(|| header_str(headers, axum::http::header::REFERER));

    if let Some(provenance) = provenance {
        return !matches_origin(provenance, base_origin, request_host);
    }

    // Some browsers omit both headers on a same-origin fetch. `Sec-Fetch-Site`
    // is browser-generated and only used as a fallback; a cross-site or absent
    // value stays external.
    !header_str(
        headers,
        axum::http::HeaderName::from_static("sec-fetch-site"),
    )
    .is_some_and(|site| site.eq_ignore_ascii_case("same-origin"))
}

fn header_str(
    headers: &axum::http::HeaderMap,
    name: axum::http::header::HeaderName,
) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// One parsed `scheme://host[:port]` origin.
struct Origin<'a> {
    scheme: &'a str,
    host: String,
    port: Option<u16>,
}

/// Parse the origin part of an absolute URL or an `Origin` header.
///
/// `Origin: null`, a relative URL, or anything without a scheme is not an
/// origin we can compare and therefore classifies as external.
fn parse_origin(value: &str) -> Option<Origin<'_>> {
    let (scheme, rest) = value.split_once("://")?;
    if scheme.is_empty() {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?.trim();
    if authority.is_empty() {
        return None;
    }
    let (host, port) = parse_authority(authority)?;
    Some(Origin { scheme, host, port })
}

/// Split an authority into a lower-case host and an optional port.
///
/// Handles `[::1]:8443`, `example.com:8443` and a bare `example.com`. An invalid
/// port is rejected instead of being silently dropped: `example.com:http` is not
/// the same origin as `example.com`.
fn parse_authority(value: &str) -> Option<(String, Option<u16>)> {
    if let Some(rest) = value.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        let port = match tail {
            "" => None,
            tail => Some(tail.strip_prefix(':')?.parse().ok()?),
        };
        return Some((host.to_ascii_lowercase(), port));
    }

    match value.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            let port = if port.is_empty() {
                None
            } else {
                Some(port.parse().ok()?)
            };
            Some((host.to_ascii_lowercase(), port))
        }
        _ => Some((value.to_ascii_lowercase(), None)),
    }
}

/// Compare a provenance URL against the configured origin and the request's
/// `Host` header.
fn matches_origin(value: &str, base_origin: Option<&str>, request_host: Option<&str>) -> bool {
    let Some(observed) = parse_origin(value) else {
        return false;
    };

    if let Some(base) = base_origin.and_then(parse_origin) {
        // `external_url` is the authoritative origin when configured: a
        // same-host request on another scheme or port is still a different
        // browser origin. Falling through to `Host` here would silently make
        // that external call look like Desk traffic.
        return observed.scheme.eq_ignore_ascii_case(base.scheme)
            && observed.host == base.host
            && effective_port(&observed) == effective_port(&base);
    }

    // Without `external_url`, the Host fallback deliberately ignores the
    // scheme: the request layer sees a path, not whether TLS terminated
    // in-process or at a reverse proxy, and a LAN install is reached by an
    // address unknown at boot.
    let Some((host, port)) = request_host.and_then(parse_authority) else {
        return false;
    };
    observed.host == host && ports_compatible(observed.port, port)
}

fn effective_port(origin: &Origin<'_>) -> Option<u16> {
    origin.port.or_else(|| {
        if origin.scheme.eq_ignore_ascii_case("http") {
            Some(80)
        } else if origin.scheme.eq_ignore_ascii_case("https") {
            Some(443)
        } else {
            None
        }
    })
}

const fn ports_compatible(observed: Option<u16>, host: Option<u16>) -> bool {
    match (observed, host) {
        (Some(a), Some(b)) => a == b,
        (None, None) => true,
        // A missing port on one side is the scheme default. We cannot know the
        // request scheme here, so accept only the standard web ports.
        (Some(port), None) | (None, Some(port)) => port == 80 || port == 443,
    }
}

/// One configured proxy: an address or a CIDR network.
#[derive(Debug, Clone, Copy)]
struct TrustedProxy {
    network: IpAddr,
    prefix: u8,
}

/// Process-wide trusted-proxy list, populated from
/// `[server] trusted_proxies` by the composition root.
///
/// It lives here as a fallback because [`record_request`] only receives
/// `AppState`, which does not carry the server settings. `router/build.rs`
/// derives `api_log_base_origin` from the same settings and is the exact place
/// that must call [`set_trusted_proxies`] before the router is built. Until it
/// does, the list is empty and every forwarding header is ignored — the safe
/// default.
static TRUSTED_PROXIES: OnceLock<RwLock<Vec<TrustedProxy>>> = OnceLock::new();

fn trusted_proxy_store() -> &'static RwLock<Vec<TrustedProxy>> {
    TRUSTED_PROXIES.get_or_init(|| RwLock::new(Vec::new()))
}

/// Replace the trusted-proxy list from `[server] trusted_proxies` values.
///
/// Accepts an exact address (`10.0.0.5`) or a CIDR network (`10.0.0.0/8`,
/// `2001:db8::/32`). Unparsable entries are skipped rather than widening the
/// trust to everything; the caller that owns boot-time validation reports them.
pub fn set_trusted_proxies<I, S>(specs: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let parsed = specs
        .into_iter()
        .filter_map(|spec| parse_trusted_proxy(spec.as_ref()))
        .collect();
    *trusted_proxy_store()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = parsed;
}

fn parse_trusted_proxy(spec: &str) -> Option<TrustedProxy> {
    let spec = spec.trim();
    let (address, prefix) = match spec.split_once('/') {
        Some((address, prefix)) => (address.trim(), Some(prefix.trim().parse::<u8>().ok()?)),
        None => (spec, None),
    };
    let network: IpAddr = address.parse().ok()?;
    let max = if network.is_ipv4() { 32 } else { 128 };
    let prefix = prefix.unwrap_or(max);
    (prefix <= max).then_some(TrustedProxy { network, prefix })
}

fn is_trusted_peer(peer: IpAddr, proxies: &[TrustedProxy]) -> bool {
    proxies.iter().any(|proxy| match (proxy.network, peer) {
        (IpAddr::V4(network), IpAddr::V4(peer)) => {
            let prefix = proxy.prefix.min(32);
            prefix == 0 || {
                let mask = u32::MAX << (32 - prefix);
                u32::from(network) & mask == u32::from(peer) & mask
            }
        }
        (IpAddr::V6(network), IpAddr::V6(peer)) => {
            let prefix = proxy.prefix.min(128);
            prefix == 0 || {
                let mask = u128::MAX << (128 - prefix);
                u128::from(network) & mask == u128::from(peer) & mask
            }
        }
        _ => false,
    })
}

/// First hop of `X-Forwarded-For`, else `X-Real-IP`.
///
/// Kept unverified on purpose: this is the raw header read the legacy tests
/// use. Audit rows must go through [`trusted_client_ip`], which consults the
/// TCP peer first.
#[cfg(test)]
fn client_ip(headers: &axum::http::HeaderMap) -> String {
    forwarded_ip(headers).unwrap_or_default()
}

fn forwarded_ip(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|value| value.to_str().ok())
        })
        .map(str::trim)
        .filter(|value| value.parse::<IpAddr>().is_ok())
        .map(str::to_string)
}

/// Address to record in an audit row: the forwarding header only when the TCP
/// peer is a configured proxy, otherwise the peer itself.
///
/// With no peer (`ConnectInfo` absent, which today is the plain-HTTP listener)
/// the headers are ignored too: an unverifiable address is worse than none,
/// because it would let the caller choose what the audit trail records
/// (SEC-08).
fn trusted_client_ip(
    headers: &axum::http::HeaderMap,
    peer: Option<IpAddr>,
    proxies: &[TrustedProxy],
) -> String {
    let Some(peer) = peer else {
        return String::new();
    };
    if is_trusted_peer(peer, proxies) {
        forwarded_ip(headers).unwrap_or_else(|| peer.to_string())
    } else {
        peer.to_string()
    }
}

/// `/api/resource/<doctype>/<name>` — the generic document read route.
///
/// Exactly two segments after the prefix. Sub-resources such as
/// `/api/resource/User/<id>/neighbors` are GETs too, but they do not open a
/// document: matching them here used to add a View Log row for a request that
/// only asked for the record's neighbours.
fn view_target(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix("/api/resource/")?;
    let mut segments = rest.split('/');
    let doctype = segments.next().filter(|part| !part.is_empty())?;
    let name = segments.next().filter(|part| !part.is_empty())?;
    if segments.next().is_some() {
        return None;
    }
    Some((doctype.to_string(), name.to_string()))
}

#[cfg(test)]
mod audit_network_tests {
    use super::*;
    use axum::http::HeaderMap;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                value.parse().expect("header value"),
            );
        }
        headers
    }

    fn proxies(specs: &[&str]) -> Vec<TrustedProxy> {
        specs
            .iter()
            .filter_map(|spec| parse_trusted_proxy(spec))
            .collect()
    }

    #[test]
    fn an_untrusted_peer_cannot_choose_its_address() {
        let headers = headers(&[
            ("x-forwarded-for", "203.0.113.7"),
            ("x-real-ip", "203.0.113.8"),
        ]);
        let peer = Some("192.0.2.9".parse().expect("ip"));
        assert_eq!(
            trusted_client_ip(&headers, peer, &proxies(&["10.0.0.0/8"])),
            "192.0.2.9",
            "a peer outside trusted_proxies is recorded, not its header"
        );
    }

    #[test]
    fn a_trusted_proxy_supplies_the_forwarded_address() {
        let forwarded = headers(&[("x-forwarded-for", "203.0.113.7, 10.0.0.1")]);
        let peer = Some("10.1.2.3".parse().expect("ip"));
        assert_eq!(
            trusted_client_ip(&forwarded, peer, &proxies(&["10.0.0.0/8"])),
            "203.0.113.7"
        );

        let real_ip = headers(&[("x-real-ip", "203.0.113.8")]);
        assert_eq!(
            trusted_client_ip(&real_ip, peer, &proxies(&["10.1.2.3"])),
            "203.0.113.8"
        );
    }

    #[test]
    fn cidr_matching_respects_the_family_and_prefix() {
        let trusted = proxies(&["10.0.0.0/8", "2001:db8::/32"]);
        assert!(is_trusted_peer("10.255.0.1".parse().expect("ip"), &trusted));
        assert!(!is_trusted_peer("11.0.0.1".parse().expect("ip"), &trusted));
        assert!(is_trusted_peer(
            "2001:db8::1".parse().expect("ip"),
            &trusted
        ));
        assert!(!is_trusted_peer(
            "2001:db9::1".parse().expect("ip"),
            &trusted
        ));
        assert!(!is_trusted_peer(
            "::ffff:10.0.0.1".parse().expect("ip"),
            &trusted
        ));
        assert!(parse_trusted_proxy("10.0.0.1/64").is_none());
        assert!(parse_trusted_proxy("not-an-ip").is_none());
    }

    #[test]
    fn without_a_connect_info_the_headers_are_ignored() {
        let headers = headers(&[("x-forwarded-for", "203.0.113.7")]);
        assert_eq!(
            trusted_client_ip(&headers, None, &proxies(&["0.0.0.0/0"])),
            "",
            "an unverifiable address must not reach the audit trail"
        );
    }

    #[test]
    fn external_url_controls_api_log_suppression() {
        let same_host = headers(&[
            ("host", "desk.example.com"),
            ("origin", "https://desk.example.com"),
        ]);

        // No configured origin: never suppress on the caller's own Host header.
        assert!(api_request_is_external(&same_host, None));

        // A configured origin still recognises the Desk's own traffic.
        assert!(!api_request_is_external(
            &same_host,
            Some("https://desk.example.com")
        ));
        assert!(api_request_is_external(
            &headers(&[("origin", "https://evil.example")]),
            Some("https://desk.example.com")
        ));
    }

    /// PERF-01: the decision that lets the logger skip credential work is
    /// purely path-based and must agree with `should_log` for every status.
    #[test]
    fn assets_never_reach_principal_resolution() {
        for path in [
            "/static/desk.css",
            "/pkg/app.wasm",
            "/plugins/billing/asset/style.css",
        ] {
            assert!(!may_log(path), "{path}");
            assert!(!should_log(path, 200), "{path}");
        }
        assert!(may_log("/api/resource/ToDo"));
        assert!(may_log("/events"), "only a successful stream is skipped");
        assert!(!should_log("/events", 200));
        assert!(should_log("/events", 403));
    }
}

#[cfg(test)]
mod tests;
