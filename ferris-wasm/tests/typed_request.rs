//! Empirical proof for the typed `handle-request` boundary (WIT 2.0.0, P1).
//!
//! 1.0 passed `(route, body: string)`: method, query, headers and binary
//! bodies were destroyed at the host boundary, and the reply was a bare
//! `String` (always 200, always `text/plain`). These tests drive the new
//! record/response pair end to end against the `probe-ok` fixture, whose
//! `/echo` route reflects the request back verbatim and whose `/teapot`
//! answers with a status and headers of its own.
//!
//! The host-side half (axum request → record, record → axum response) is
//! covered by `ferris-server`'s and `ferris-web`'s `plugin_request_tests`
//! and by `ferris_wasm::http::sanitize_response_headers`'s unit tests.
//!
//! Build the fixture first (it skips — loudly — when absent):
//! ```sh
//! cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml
//! ```

use std::path::{Path, PathBuf};

use ferris_core::{DatabaseConfig, DatabaseType, SslMode};
use ferris_db::create_pool;
use ferris_wasm::{HttpRequest, PluginStore, RuntimeConfig, SharedRuntime};

/// One dev server's plugin store on in-memory SQLite. Epoch interrupts stay
/// off: nothing ticks the epoch in a test, so the call deadlines never fire.
async fn store() -> PluginStore {
    let runtime = std::sync::Arc::new(
        SharedRuntime::new(RuntimeConfig {
            enable_epoch_interrupts: false,
            ..Default::default()
        })
        .expect("SharedRuntime"),
    );
    let pool = create_pool(&DatabaseConfig {
        r#type: DatabaseType::SQLite,
        host: "localhost".into(),
        port: 0,
        name: ":memory:".into(),
        user: String::new(),
        password: String::new(),
        max_connections: 5,
        min_connections: 1,
        connect_timeout_secs: 5,
        idle_timeout_secs: 300,
        max_lifetime_secs: 3600,
        ssl_mode: SslMode::Disable,
        schema: None,
    })
    .await
    .expect("in-memory sqlite pool");
    PluginStore::new(runtime, pool)
}

fn probe_ok() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/probe-ok/target/wasm32-wasip2/debug/probe_ok.wasm")
}

/// Load `probe-ok` or skip loudly (fresh clone without prebuilt fixtures).
async fn loaded_store() -> Option<PluginStore> {
    if !probe_ok().exists() {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml"
        );
        return None;
    }
    let store = store().await;
    store
        .swap_load_with_name(probe_ok(), "probe")
        .expect("probe-ok must instantiate (no filesystem/sockets in its imports)");
    Some(store)
}

/// The full request crosses intact — method, query, header values and a
/// **binary** body (bytes that are not valid UTF-8) — and the response's
/// status, headers and body come back the same way.
#[tokio::test]
async fn request_crosses_and_response_passes_back() {
    let Some(store) = loaded_store().await else {
        return;
    };
    let raw_body = vec![0u8, 159, 146, 150, b'O', b'K', 0xff];
    let request = HttpRequest {
        method: "PATCH".to_string(),
        path: "/echo".to_string(),
        query: "a=1&b=héllo+world".to_string(),
        headers: vec![
            ("x-probe-key".to_string(), "yes".to_string()),
            ("accept".to_string(), "text/plain".to_string()),
        ],
        body: raw_body.clone(),
        client_identity: None,
    };

    let resp = store
        .handle_request("probe", &request)
        .expect("/echo answers");

    assert_eq!(resp.status, 200);
    assert!(
        resp.headers
            .contains(&("x-probe".to_string(), "echo".to_string())),
        "response headers lost: {:?}",
        resp.headers
    );

    let mut expected = "PATCH|a=1&b=héllo+world|yes|".to_string().into_bytes();
    expected.extend(raw_body);
    assert_eq!(
        resp.body, expected,
        "the request did not cross verbatim (method/query/header/body)"
    );
}

/// The guest owns the response: an unusual status, its own content-type and
/// a binary body survive the host boundary (1.x could only ever answer
/// 200/text-plain).
#[tokio::test]
async fn guest_chooses_status_headers_and_body() {
    let Some(store) = loaded_store().await else {
        return;
    };
    let resp = store
        .handle_request(
            "probe",
            &HttpRequest {
                method: "GET".to_string(),
                path: "/teapot".to_string(),
                query: String::new(),
                headers: vec![],
                body: vec![],
                client_identity: None,
            },
        )
        .expect("/teapot answers");

    assert_eq!(resp.status, 418);
    assert_eq!(
        resp.headers,
        vec![(
            "content-type".to_string(),
            "text/teapot; charset=utf-8".to_string()
        )]
    );
    assert_eq!(resp.body, b"short and stout".to_vec());
}

/// The typed error path still maps (unknown route → `not-found`), i.e. the
/// record change did not collapse errors into 200s.
#[tokio::test]
async fn unknown_route_still_errors() {
    let Some(store) = loaded_store().await else {
        return;
    };
    let err = store
        .handle_request(
            "probe",
            &HttpRequest {
                method: "GET".to_string(),
                path: "/nope".to_string(),
                query: String::new(),
                headers: vec![],
                body: vec![],
                client_identity: None,
            },
        )
        .expect_err("/nope must not answer");
    assert!(
        format!("{err}").contains("unknown route"),
        "unexpected error: {err}"
    );
}

/// SRV17 regression: loading a second app must not evict the first. Before the
/// registry, `PluginStore` kept one `current`, so `/plugins/{first}` and
/// `/api/method/<first method>` answered 404 as soon as another app loaded.
#[tokio::test]
async fn loading_a_second_app_keeps_the_first_addressable() {
    if !probe_ok().exists() {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml"
        );
        return;
    }
    let store = store().await;
    store
        .swap_load_with_name(probe_ok(), "probe-a")
        .expect("first probe must instantiate");
    store
        .swap_load_with_name(probe_ok(), "probe-b")
        .expect("second probe must instantiate");

    assert_eq!(store.plugins().len(), 2, "both apps stay registered");
    assert!(store.get("probe-a").is_some(), "first app must stay loaded");
    assert!(store.get("probe-b").is_some(), "second app must be loaded");
    assert_eq!(
        store.name().as_deref(),
        Some("probe-b"),
        "the most recent swap stays the default"
    );

    // Dispatching to the *older* app still reaches a guest, which is the
    // behaviour multi-app installs need.
    let resp = store
        .handle_request(
            "probe-a",
            &HttpRequest {
                method: "GET".to_string(),
                path: "/echo".to_string(),
                query: String::new(),
                headers: vec![],
                body: vec![],
                client_identity: None,
            },
        )
        .expect("probe-a must still answer after probe-b loaded");
    assert_eq!(resp.status, 200);
}
