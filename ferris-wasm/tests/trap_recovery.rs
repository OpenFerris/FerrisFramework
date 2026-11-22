//! Regression test for trap recovery in [`PluginStore`].
//!
//! A wasm trap leaves the `wasmtime::Store` poisoned — wasmtime refuses to
//! re-enter that instance (`cannot enter component instance`), so **every**
//! later call on the plugin fails until the store swaps in a fresh instance.
//! That is the "wedge" a single trapped frame used to cause. This test proves
//! the store rebuilds the component from the bytes it remembers and that the
//! next request succeeds.
//!
//! The fixture is a separate crate, built on demand:
//!
//! ```sh
//! cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-trap/Cargo.toml
//! ```
//!
//! and the test skips — loudly, on stdout — when the component is absent, so a
//! fresh clone stays green without prebuilt binaries in the repository.
//!
//! Without the fix (`PluginStore::recover_after_trap` absent and the entry
//! points not guarded) the `loaded()` pointer never changes and `/ping` after
//! the trap fails, so both assertions below fail.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferris_core::{DatabaseConfig, DatabaseType, SslMode};
use ferris_db::create_pool;
use ferris_wasm::{HttpRequest, PluginStore, RuntimeConfig, SharedRuntime};

/// One dev server's plugin store. Epoch interrupts stay off: nothing ticks the
/// epoch in a test, so the load/call deadlines never fire and the trap comes
/// solely from the guest's explicit `unreachable`.
async fn store() -> PluginStore {
    let runtime = Arc::new(
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

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// The typed request the 2.0 contract hands the guest: a plain GET.
fn get(path: &str) -> HttpRequest {
    HttpRequest {
        method: "GET".to_string(),
        path: path.to_string(),
        query: String::new(),
        headers: vec![],
        body: vec![],
        client_identity: None,
    }
}

/// The response body as text (every route this test calls answers UTF-8).
fn body(resp: ferris_wasm::Response) -> String {
    String::from_utf8(resp.body).expect("fixture response body is UTF-8")
}

#[tokio::test]
async fn a_trapped_instance_is_replaced_and_the_next_request_succeeds() {
    let path = fixture("tests/fixtures/probe-trap/target/wasm32-wasip2/debug/probe_trap.wasm");
    if !path.exists() {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-trap/Cargo.toml"
        );
        return;
    }
    let store = store().await;
    store
        .swap_load_with_name(&path, "probe")
        .expect("probe-trap must instantiate");
    let before = store.loaded().expect("plugin loaded before the trap");

    // A sane instance answers.
    assert_eq!(
        body(
            store
                .handle_request("probe", &get("/ping"))
                .expect("ping before the trap"),
        ),
        "pong"
    );

    // The guest traps deterministically on `/trap`.
    let trapped = store.handle_request("probe", &get("/trap"));
    let err = trapped.expect_err("/trap must raise a wasm trap");
    assert!(
        format!("{err}").contains("trap"),
        "expected a trap error, got: {err}"
    );

    // The store must not stay wedged: it publishes a fresh instance…
    let after = store.loaded().expect("plugin loaded after the trap");
    assert!(
        !Arc::ptr_eq(&before, &after),
        "trap recovery must replace the poisoned instance with a fresh one"
    );

    // …and the fresh instance serves again.
    assert_eq!(
        body(
            store
                .handle_request("probe", &get("/ping"))
                .expect("ping after the trap"),
        ),
        "pong"
    );
}

#[tokio::test]
async fn recovery_is_repeatable_after_repeated_traps() {
    let path = fixture("tests/fixtures/probe-trap/target/wasm32-wasip2/debug/probe_trap.wasm");
    if !path.exists() {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-trap/Cargo.toml"
        );
        return;
    }
    let store = store().await;
    store
        .swap_load_with_name(&path, "probe")
        .expect("probe-trap must instantiate");

    // Two trap→recover rounds: the store keeps recovering rather than
    // degrading into a silently-dead handler, and each recovered instance is a
    // fresh one (not a reuse of the previous Arc).
    let mut previous = store.loaded().expect("first plugin");
    for _ in 0..2 {
        assert!(
            store.handle_request("probe", &get("/trap")).is_err(),
            "/trap must keep trapping"
        );
        let current = store.loaded().expect("recovered plugin");
        assert!(
            !Arc::ptr_eq(&previous, &current),
            "each trap must publish a fresh instance"
        );
        assert_eq!(
            body(
                store
                    .handle_request("probe", &get("/ping"))
                    .expect("ping after recovery"),
            ),
            "pong"
        );
        previous = current;
    }
}
