//! The `handle-job` callback (WIT 4.0.0): a scheduled/enqueued job re-enters the
//! guest.
//!
//! Before `handle-job`, a plugin could register a cron schedule (or `enqueue` a
//! one-shot job) but nothing ever ran the app's code for it — a self-refresh
//! loop had to hang off a read. These tests drive the callback end to end
//! against the `probe-ok` fixture, whose `handle-job` echoes
//! `"{job-type}|{payload}"`, and pin the no-fallback contract: a store with
//! nothing loaded refuses the job instead of silently dropping it.
//!
//! Build the fixture first (it skips — loudly — when absent):
//! ```sh
//! cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml
//! ```

use std::path::{Path, PathBuf};

use ferris_core::{DatabaseConfig, DatabaseType, SslMode};
use ferris_db::create_pool;
use ferris_wasm::{PluginStore, RuntimeConfig, SharedRuntime};

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
        .expect("probe-ok must instantiate");
    Some(store)
}

/// The `job-type` and the JSON `payload` cross to the guest intact and its
/// result string comes back — the round trip a cron needs.
#[tokio::test]
async fn job_crosses_and_result_returns() {
    let Some(store) = loaded_store().await else {
        return;
    };
    let out = store
        .handle_job("task_nightly", "{\"doctype\":\"Task\"}")
        .expect("handle-job answers");
    assert_eq!(
        out, "task_nightly|{\"doctype\":\"Task\"}",
        "job-type/payload did not cross verbatim"
    );
}

/// No fallback: a claim while nothing is loaded fails loudly (the worker
/// retries, then dead-letters) rather than being silently dropped.
#[tokio::test]
async fn no_plugin_loaded_is_a_loud_error() {
    let store = store().await;
    let err = store
        .handle_job("task_nightly", "{}")
        .expect_err("an empty store must refuse the job");
    assert!(
        format!("{err}").contains("no plugin loaded"),
        "unexpected error: {err}"
    );
}
