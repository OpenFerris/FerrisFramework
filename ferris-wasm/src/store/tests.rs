use std::sync::Arc;

use ferris_core::hooks::HookPoint;

use super::events::DISPATCHING;
use super::hooks::{EVENT_POINTS, aborts_on_error};
use super::registry::PluginStore;
use crate::runtime::{RuntimeConfig, SharedRuntime};

// `#[tokio::test]`, not `Handle::current().block_on(...)` in a plain `#[test]`:
// there is no reactor in a plain test, so that call panicked before it could
// assert anything, which is presumably why the test was ignored instead of
// fixed.
#[tokio::test]
async fn empty_store_has_no_plugin() {
    let runtime = Arc::new(SharedRuntime::default());
    let pool = ferris_db::create_pool(&ferris_core::DatabaseConfig {
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
    .unwrap();
    let store = PluginStore::new(runtime, pool);
    assert!(store.loaded().is_none());
    assert!(store.name().is_none());
    assert!(store.describe().is_err());
}

/// The Fase 1 asset channel guards its store access the same way the
/// request handlers do: no plugin loaded → typed errors, not panics.
#[tokio::test]
async fn empty_store_cannot_serve_ui_assets_or_widgets() {
    let store = PluginStore::new(Arc::new(SharedRuntime::default()), memory_pool().await);
    assert!(store.ui_asset("test", "ui/app.js").is_err());
    assert!(store.ui_widget("test", "open-tasks", &[]).is_err());
}

/// §C4 regression: the seam `ferris_server::Server::run` calls at boot must
/// genuinely interrupt a guest that never returns. The host arms a finite
/// epoch deadline around every plugin call, and the clock started here is
/// what fires those deadlines — before C4 no production path started the
/// clock, so the same guest hung its worker forever.
#[tokio::test]
async fn c4_a_started_epoch_clock_interrupts_a_runaway_guest() {
    let runtime = Arc::new(
        SharedRuntime::new(RuntimeConfig {
            // A 1 ms tick keeps the trap fast without racing the guest.
            epoch_increment_ms: 1,
            ..Default::default()
        })
        .unwrap(),
    );
    let store = PluginStore::new(runtime.clone(), memory_pool().await);

    // The boot seam `Server::run` uses:
    let clock = store
        .start_epoch_clock()
        .expect("a store with epoch interrupts enabled must start the clock");
    // The clock task runs for the process lifetime, not until its handle
    // is dropped.
    assert!(!clock.is_finished());

    // A guest that never returns, under the same deadline the host arms on
    // every plugin call (`Plugin::invoke`).
    let spin = wat::parse_str(
        r#"(module
            (func $busy nop)
            (func (export "spin")
                (loop (call $busy) (br 0))))"#,
    )
    .expect("sample wat");
    let module = wasmtime::Module::new(runtime.engine(), spin).expect("compile spin");

    let engine = runtime.engine().clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let mut store = wasmtime::Store::new(&engine, ());
        store.set_epoch_deadline(1);
        let linker = wasmtime::Linker::new(&engine);
        let instance = linker
            .instantiate(&mut store, &module)
            .expect("instantiate spin");
        let spin = instance
            .get_typed_func::<(), ()>(&mut store, "spin")
            .expect("typed func");
        // The deadline only fires because the clock keeps bumping the
        // engine's epoch from the tokio runtime this test drives.
        let _ = tx.send(spin.call(&mut store, ()));
    });

    match tokio::time::timeout(std::time::Duration::from_secs(10), rx).await {
        Ok(Ok(Err(_))) => {}
        Ok(Ok(Ok(()))) => {
            panic!("C4 regression: the runaway guest completed without being interrupted")
        }
        Ok(Err(_)) => panic!("spin thread ended without reporting"),
        Err(e) => {
            panic!("C4 regression: the runaway guest is still running — no live epoch clock ({e})")
        }
    }
}

/// The flip side of the same seam: no interrupts, no clock.
#[tokio::test]
async fn c4_a_store_with_interrupts_disabled_starts_no_clock() {
    let runtime = Arc::new(
        SharedRuntime::new(RuntimeConfig {
            enable_epoch_interrupts: false,
            ..Default::default()
        })
        .unwrap(),
    );
    let store = PluginStore::new(runtime, memory_pool().await);
    assert!(store.start_epoch_clock().is_none());
}

/// An in-memory pool for tests that need a `PluginStore` but no schema.
async fn memory_pool() -> ferris_db::Db {
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
    .unwrap()
}

/// The `probe-ok` component, when the fixture has been built. Tests skip
/// loudly otherwise, matching the integration suites.
fn probe_ok() -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/probe-ok/target/wasm32-wasip2/debug/probe_ok.wasm");
    if path.exists() {
        Some(path)
    } else {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 \
             --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml"
        );
        None
    }
}

async fn fixture_store() -> PluginStore {
    // No clock ticks the epoch in a test, so leave interrupts off: the
    // fixture never traps and a deadline would only add flakiness.
    let runtime = Arc::new(
        SharedRuntime::new(RuntimeConfig {
            enable_epoch_interrupts: false,
            ..Default::default()
        })
        .expect("SharedRuntime"),
    );
    PluginStore::new(runtime, memory_pool().await)
}

/// M: registration walks `plugins()`, so its order must not depend on
/// `HashMap` iteration. Two loads in the "wrong" order must still come back
/// sorted, or a name collision depends on process luck.
#[tokio::test]
async fn plugins_are_stable_ordered_by_name() {
    let Some(path) = probe_ok() else {
        return;
    };
    let store = fixture_store().await;
    store
        .swap_load_with_name(&path, "b-app")
        .expect("load b-app");
    store
        .swap_load_with_name(&path, "a-app")
        .expect("load a-app");
    let names: Vec<String> = store
        .plugins()
        .iter()
        .map(|p| p.name().to_string())
        .collect();
    assert_eq!(names, vec!["a-app", "b-app"]);
}

/// M: a `before-*` raised while the single guest instance cannot be
/// re-entered is refused with a typed validation error; it is no longer
/// silently skipped, and the message names the explicit opt-out.
#[tokio::test]
async fn before_event_that_cannot_dispatch_is_an_error() {
    let Some(path) = probe_ok() else {
        return;
    };
    let store = fixture_store().await;
    store
        .swap_load_with_name(&path, "probe-ok")
        .expect("load probe-ok");

    // The normal path still reaches the guest.
    store
        .on_doctype_event_pre("before-save", "Probe", "P-1", "{}", None)
        .expect("the guest accepts the event");

    // Model the reentrant path: `on_doctype_event` sets this flag while it
    // dispatches, so a nested `before-*` cannot run.
    DISPATCHING.with(|d| d.set(true));
    let err = store
        .on_doctype_event_pre("before-save", "Probe", "P-1", "{}", None)
        .expect_err("a before-* that cannot run must not succeed");
    DISPATCHING.with(|d| d.set(false));
    assert!(
        matches!(
            err,
            crate::WasmError::PluginError {
                kind: crate::PluginErrorKind::ValidationFailed,
                ..
            }
        ),
        "expected validation-failed, got {err:?}"
    );
    assert!(
        err.to_string().contains("write-no-events"),
        "the refusal must name the opt-out: {err}"
    );
}

/// R-3 / R-9: the exact lifecycle contract the WIT world documents. Every
/// `HookPoint` the engine knows is forwarded exactly once, under the event
/// name the WIT table lists; insert still emits `created` (never
/// `after-save`), and every pre-write point aborts on a guest error.
#[test]
fn lifecycle_points_match_the_documented_contract() {
    let events: Vec<&str> = EVENT_POINTS.iter().map(|(_, event)| *event).collect();
    assert_eq!(
        events,
        vec![
            "before-naming",
            "before-validate",
            "validate",
            "before-insert",
            "created",
            "before-save",
            "after-save",
            "on-update",
            "before-submit",
            "submitted",
            "before-cancel",
            "cancelled",
            "before-update-after-submit",
            "on-update-after-submit",
            "on-change",
            "before-delete",
            "trashed",
            "before-rename",
            "after-rename",
        ]
    );
    assert_eq!(
        EVENT_POINTS
            .iter()
            .find(|(point, _)| *point == HookPoint::AfterInsert)
            .map(|(_, event)| *event),
        Some("created"),
        "insert must emit `created`, not `after-save`"
    );
    // Every engine point is forwarded once, and the abortive set is exactly
    // the pre-write set (including `validate`, which has no `before-` name).
    assert_eq!(
        EVENT_POINTS.len(),
        19,
        "all `HookPoint` variants must reach the guest"
    );
    assert_eq!(
        EVENT_POINTS
            .iter()
            .filter(|(point, _)| aborts_on_error(*point))
            .map(|(_, event)| *event)
            .collect::<Vec<_>>(),
        vec![
            "before-naming",
            "before-validate",
            "validate",
            "before-insert",
            "before-save",
            "before-submit",
            "before-cancel",
            "before-update-after-submit",
            "before-delete",
            "before-rename",
        ],
        "the abortive points are exactly the pre-write ones"
    );
}
