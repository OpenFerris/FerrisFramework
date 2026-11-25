//! Regression tests for the observability producers.

use super::{
    Mirrors, client_ip, is_external_api_request, should_log, system_ctx, upsert, view_target,
    write_doc,
};
use ferris_core::Value;
use ferris_db::Db;
use ferris_document::engine::{DocumentEngine, ListQuery};

#[test]
fn view_target_only_matches_document_reads() {
    assert_eq!(
        view_target("/api/resource/ToDo/abc"),
        Some(("ToDo".to_string(), "abc".to_string()))
    );
    assert_eq!(view_target("/api/resource/ToDo"), None);
    assert_eq!(view_target("/api/resource/ToDo/"), None);
    assert_eq!(view_target("/api/resource/ToDo/abc/neighbors"), None);
    assert_eq!(view_target("/api/resource/ToDo/abc/comments"), None);
    assert_eq!(view_target("/api/resource/"), None);
    assert_eq!(view_target("/api/logout"), None);
}

#[test]
fn asset_prefixes_are_not_logged() {
    assert!(!should_log("/static/desk.css", 200));
    assert!(!should_log("/pkg/ferris-web.js", 200));
    assert!(!should_log(
        "/plugins/solmecbox/asset/ui/rule-chain/index.js",
        200
    ));
    assert!(!should_log("/plugins/solmecbox/asset/ui/missing.js", 404));
    assert!(should_log("/plugins/solmecbox/telemetry/stats", 403));
    assert!(should_log("/api/resource/ToDo", 200));
    assert!(should_log("/desk", 200));
}

#[test]
fn successful_streams_are_skipped_but_rejections_are_logged() {
    assert!(!should_log("/events", 200));
    assert!(!should_log("/events?channel=user:1", 200));
    assert!(!should_log("/socket.io/", 101));
    assert!(!should_log("/api/notifications/stream", 200));
    assert!(should_log("/events", 403));
    assert!(should_log("/events", 401));
    assert!(should_log("/socket.io/", 400));
    assert!(should_log("/api/notifications/stream", 401));
}

#[test]
fn api_request_log_keeps_external_callers_and_skips_the_desk() {
    use axum::http::header::{HOST, ORIGIN, REFERER};

    fn headers(pairs: &[(&str, &str)]) -> axum::http::HeaderMap {
        let mut headers = axum::http::HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
                value.parse().expect("valid header value"),
            );
        }
        headers
    }

    let base = Some("https://desk.example.com");

    // No browser provenance at all is an external API client (curl, device,
    // webhook), which is exactly what this log must keep.
    assert!(is_external_api_request(&headers(&[]), base));

    // The Desk's own fetches: same-origin origin or referer.
    assert!(!is_external_api_request(
        &headers(&[(ORIGIN.as_str(), "https://desk.example.com")]),
        base
    ));
    assert!(!is_external_api_request(
        &headers(&[(REFERER.as_str(), "https://desk.example.com/desk")]),
        base
    ));
    // A default port is the same origin even when one side omits it.
    assert!(!is_external_api_request(
        &headers(&[(ORIGIN.as_str(), "https://desk.example.com:443")]),
        base
    ));

    // A foreign origin, a foreign port or `Origin: null` is external.
    assert!(is_external_api_request(
        &headers(&[(ORIGIN.as_str(), "https://evil.example")]),
        base
    ));
    assert!(is_external_api_request(
        &headers(&[(ORIGIN.as_str(), "https://desk.example.com:8444")]),
        base
    ));
    assert!(is_external_api_request(
        &headers(&[(ORIGIN.as_str(), "null")]),
        base
    ));
    // The scheme is part of the configured origin, even when the request's
    // Host header happens to carry the same host name.
    assert!(is_external_api_request(
        &headers(&[
            (HOST.as_str(), "desk.example.com"),
            (ORIGIN.as_str(), "http://desk.example.com"),
        ]),
        base
    ));

    // Without `external_url`, the request's Host is the fallback (LAN installs
    // bind 0.0.0.0 and are reached by an address unknown at boot).
    assert!(!is_external_api_request(
        &headers(&[
            (HOST.as_str(), "192.168.1.10:8443"),
            (ORIGIN.as_str(), "https://192.168.1.10:8443"),
        ]),
        None
    ));

    // When a browser omits both headers, its fetch metadata is the fallback.
    assert!(!is_external_api_request(
        &headers(&[
            (HOST.as_str(), "desk.example.com"),
            ("sec-fetch-site", "same-origin"),
        ]),
        base
    ));
    assert!(is_external_api_request(
        &headers(&[
            (HOST.as_str(), "desk.example.com"),
            ("sec-fetch-site", "cross-site"),
        ]),
        base
    ));
}

#[test]
fn forwarded_for_takes_the_first_hop() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-forwarded-for", "203.0.113.7, 10.0.0.1".parse().unwrap());
    headers.insert("x-real-ip", "10.0.0.9".parse().unwrap());
    assert_eq!(client_ip(&headers), "203.0.113.7");
}

#[test]
fn no_forwarding_header_yields_empty_ip() {
    assert_eq!(client_ip(&axum::http::HeaderMap::new()), "");
}

async fn memory_pool() -> Db {
    ferris_db::create_pool(&ferris_core::DatabaseConfig {
        r#type: ferris_core::DatabaseType::SQLite,
        host: "localhost".into(),
        port: 0,
        name: ":memory:".into(),
        user: String::new(),
        password: String::new(),
        max_connections: 1,
        min_connections: 1,
        connect_timeout_secs: 5,
        idle_timeout_secs: 300,
        max_lifetime_secs: 3600,
        ssl_mode: ferris_core::SslMode::Disable,
        schema: None,
    })
    .await
    .expect("memory pool")
}

/// SRV7 regression: the declared `Path Log` document must actually receive a
/// row through the engine (the declaration-only state this module fixes).
#[tokio::test]
async fn path_log_rows_are_persisted_through_the_engine() {
    crate::doctypes::register_doctypes();
    let pool = memory_pool().await;
    write_doc(
        &pool,
        "Path Log",
        serde_json::json!({
            "method": "GET",
            "path": "/api/resource/ToDo/abc",
            "status_code": 200,
            "duration_ms": 3,
            "user": "Administrator",
            "ip_address": "127.0.0.1",
        }),
    )
    .await
    .expect("Path Log row is written");

    let engine = DocumentEngine::new(pool.clone());
    let ctx = system_ctx("Path Log");
    let (rows, total) = engine
        .list(&ctx, &ListQuery::default())
        .await
        .expect("Path Log can be listed");
    assert_eq!(total, 1);
    assert_eq!(
        rows[0].get("path").and_then(Value::as_str),
        Some("/api/resource/ToDo/abc")
    );
}

/// Retention deletes only `Path Log` rows older than the cutoff. `Error Log`
/// and the other audit tables are deliberately outside its scope.
#[tokio::test]
async fn retention_removes_only_rows_older_than_the_cutoff() {
    crate::doctypes::register_doctypes();
    let pool = memory_pool().await;
    let now = ferris_document::chrono::Utc::now().timestamp();
    let old = write_doc(
        &pool,
        "Path Log",
        serde_json::json!({
            "method": "GET",
            "path": "/old",
            "status_code": 200,
            "duration_ms": 1,
            "user": "Administrator",
            "ip_address": "",
        }),
    )
    .await
    .expect("old Path Log row");
    let recent = write_doc(
        &pool,
        "Path Log",
        serde_json::json!({
            "method": "GET",
            "path": "/recent",
            "status_code": 200,
            "duration_ms": 1,
            "user": "Administrator",
            "ip_address": "",
        }),
    )
    .await
    .expect("recent Path Log row");

    // The engine stamped both rows with `now`; rewrite just the column under
    // test, the same one retention compares against.
    set_creation(&pool, &old, now - 40 * 86_400).await;
    set_creation(&pool, &recent, now - 1 * 86_400).await;

    let deleted = super::logs::purge_older_than(&pool, now - 30 * 86_400)
        .await
        .expect("retention runs");
    assert_eq!(deleted, 1, "only the row past the cutoff was deleted");

    let engine = DocumentEngine::new(pool.clone());
    let ctx = system_ctx("Path Log");
    let (rows, total) = engine
        .list(&ctx, &ListQuery::default())
        .await
        .expect("Path Log can be listed");
    assert_eq!(total, 1);
    assert_eq!(
        rows[0].get("name").and_then(Value::as_str),
        Some(recent.as_str())
    );
}

async fn set_creation(pool: &Db, name: &str, at: i64) {
    let table = ferris_schema::migration::document_table("Path Log").expect("Path Log table");
    let quoted = ferris_db::sql::Ident::new(&table)
        .expect("valid table identifier")
        .quote(pool.dialect());
    pool.execute(&ferris_db::Stmt::new(
        format!("UPDATE {quoted} SET creation = ? WHERE name = ?"),
        vec![Value::from(at), Value::from(name)],
    ))
    .await
    .expect("creation column updated");
}

/// SRV7 regression: the mirror path creates once and updates on the next tick
/// instead of duplicating the row.
#[tokio::test]
async fn mirrors_update_in_place() {
    crate::doctypes::register_doctypes();
    let pool = memory_pool().await;
    let mut mirrors = Mirrors::default();
    for rows in [1_i64, 2_i64] {
        upsert(
            &pool,
            &mut mirrors,
            "RQ Job:job-1".to_string(),
            "RQ Job",
            serde_json::json!({
                "job_id": "job-1",
                "queue": "default",
                "status": "Queued",
                "result": rows,
            }),
        )
        .await;
    }

    let engine = DocumentEngine::new(pool.clone());
    let ctx = system_ctx("RQ Job");
    let (rows, total) = engine
        .list(&ctx, &ListQuery::default())
        .await
        .expect("RQ Job can be listed");
    assert_eq!(total, 1, "the second tick updates, it does not insert");
    assert_eq!(
        rows[0].get("result").and_then(Value::as_str),
        Some("2"),
        "the update carried the newest value"
    );
}

/// SRV7 regression: a mirror whose document name is stable across restarts
/// (`Scheduled Job Type` uses the schedule name) must update the existing row
/// instead of failing every tick on the primary-key conflict.
#[tokio::test]
async fn a_stable_named_mirror_updates_after_a_restart() {
    crate::doctypes::register_doctypes();
    let pool = memory_pool().await;
    let data = |cron: &str| {
        serde_json::json!({
            "name": "nightly-cleanup",
            "cron_format": cron,
            "disabled": false,
            "stopped": false,
        })
    };

    upsert(
        &pool,
        &mut Mirrors::default(),
        "Scheduled Job Type:s1".to_string(),
        "Scheduled Job Type",
        data("0 0 * * *"),
    )
    .await;
    // A fresh process: the mirror map is empty, but the document already
    // exists under the same stable name.
    upsert(
        &pool,
        &mut Mirrors::default(),
        "Scheduled Job Type:s1".to_string(),
        "Scheduled Job Type",
        data("0 1 * * *"),
    )
    .await;

    let engine = DocumentEngine::new(pool.clone());
    let ctx = system_ctx("Scheduled Job Type");
    let (rows, total) = engine
        .list(&ctx, &ListQuery::default())
        .await
        .expect("Scheduled Job Type can be listed");
    assert_eq!(total, 1, "the restart reused the stable document name");
    assert_eq!(
        rows[0].get("cron_format").and_then(Value::as_str),
        Some("0 1 * * *"),
        "the second process refreshed the existing row"
    );
}

/// The operational endpoints compute their payload on demand; the health probe
/// is the cheap `SELECT 1` the old per-tick `System Health Report` row used to
/// persist, without the table or the write.
#[tokio::test]
async fn the_live_health_snapshot_reports_the_database_and_version() {
    let pool = ferris_db::memory_db().await.unwrap();
    let snapshot = super::health_snapshot(&pool).await;
    assert_eq!(
        snapshot.get("status").and_then(serde_json::Value::as_str),
        Some("Healthy")
    );
    assert_eq!(
        snapshot.get("database").and_then(serde_json::Value::as_str),
        Some("ok")
    );
    assert_eq!(
        snapshot.get("version").and_then(serde_json::Value::as_str),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert!(
        snapshot
            .get("checked_at")
            .and_then(serde_json::Value::as_str)
            .is_some()
    );
}

/// The storage snapshot answers with the same shape the mirror rows had, so a
/// caller can drop the stored table and read this instead.
#[tokio::test]
async fn the_live_storage_snapshot_lists_tables() {
    let pool = ferris_db::memory_db().await.unwrap();
    let tables = super::storage_snapshot(&pool).await.expect("storage query");
    // SQLite always has its schema table; the assertion is about the shape, not
    // the count.
    assert!(tables.iter().all(|table| !table.name.is_empty()));
}
