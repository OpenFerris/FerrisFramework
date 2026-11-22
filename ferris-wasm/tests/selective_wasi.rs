//! Empirical proof for the selective WASI linker.
//!
//! `plugin::add_selective_wasi` links clocks/random/stdio/environment but no
//! filesystem and no sockets. These tests prove the claims below against real
//! components; the fixtures are separate crates with their own `[workspace]`,
//! built on demand:
//!
//! ```sh
//! cargo build --target wasm32-wasip2 --manifest-path test/Cargo.toml
//! cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml
//! cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-fs/Cargo.toml
//! ```
//!
//! Each test skips — loudly, on stdout — when its component is absent, so a
//! fresh clone stays green without prebuilt binaries in the repository.

use std::path::{Path, PathBuf};

use ferris_core::{DatabaseConfig, DatabaseType, SslMode};
use ferris_db::create_pool;
use ferris_wasm::{HttpRequest, PluginStore, Response, RuntimeConfig, SharedRuntime};

/// One dev server's plugin store. Epoch interrupts stay off: nothing ticks the
/// epoch in a test, so the load/call deadlines never fire.
async fn store() -> PluginStore {
    // The reference guest now exercises its world (it writes and reads a file),
    // so the shared store is mounted on one. The world is an internal grant:
    // every app gets its own directory, and the preopen is the boundary.
    let worlds = std::env::temp_dir().join(format!(
        "ferris-worlds-test-{}-{}",
        std::process::id(),
        "shared"
    ));
    let _ = std::fs::remove_dir_all(&worlds);
    std::fs::create_dir_all(&worlds).expect("worlds root");
    store_with_worlds_opt(Some(worlds)).await
}

/// The same store, mounted on an explicit worlds root.
async fn store_with_worlds(worlds: std::path::PathBuf) -> PluginStore {
    store_with_worlds_opt(Some(worlds)).await
}

async fn store_with_worlds_opt(worlds: Option<std::path::PathBuf>) -> PluginStore {
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
    match worlds {
        Some(dir) => PluginStore::new(runtime, pool).with_worlds(dir),
        None => PluginStore::new(runtime, pool),
    }
}

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// The typed request the 2.0 contract hands the guest: a plain GET with no
/// query, headers or body.
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

/// The response body as text (every route these tests call answers UTF-8).
fn body(resp: Response) -> String {
    String::from_utf8(resp.body).expect("fixture response body is UTF-8")
}

/// (a) The reference guest — the component whose import list documents the
/// unconditional std imports (`wasi:cli/environment`, `wasi:cli/exit`,
/// terminal stdio) — instantiates and answers under the selective linker.
#[tokio::test]
async fn reference_component_instantiates_and_describes() {
    let path = fixture("../test/target/wasm32-wasip2/debug/test.wasm");
    if !path.exists() {
        eprintln!(
            "SKIPPED: build the reference guest first: cargo build --target wasm32-wasip2 --manifest-path test/Cargo.toml"
        );
        return;
    }
    let store = store().await;
    store
        .swap_load_with_name(&path, "probe")
        .expect("reference component must instantiate with the selective WASI linker");
    let desc = store
        .describe()
        .expect("describe() over the selective linker");
    assert!(
        !desc.workspaces.is_empty(),
        "reference guest must declare workspaces, got {desc:?}"
    );
}

/// (b) The interfaces the std reaches **on demand** stay linked: `SystemTime`
/// and `chrono` (wasi:clocks/wall-clock) and `uuid` v4 (wasi:random).
#[tokio::test]
async fn system_time_and_uuid_keep_working() {
    let path = fixture("tests/fixtures/probe-ok/target/wasm32-wasip2/debug/probe_ok.wasm");
    if !path.exists() {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml"
        );
        return;
    }
    let store = store().await;
    store
        .swap_load_with_name(&path, "probe")
        .expect("probe-ok must instantiate (wall-clock + random must be linked)");

    let time = body(
        store
            .handle_request("probe", &get("/time"))
            .expect("SystemTime + chrono inside the guest"),
    );
    let (unix_ms, chrono_ms) = time.split_once('|').expect("`unix|chrono` format");
    let unix_ms: u128 = unix_ms.parse().expect("unix ms is a number");
    let chrono_ms: i64 = chrono_ms.parse().expect("chrono ms is a number");
    assert!(
        chrono_ms > 1_600_000_000_000,
        "wall clock looks wrong: {time}"
    );
    let skew = unix_ms.abs_diff(u128::from(chrono_ms.unsigned_abs()));
    assert!(
        skew < 60_000,
        "SystemTime/chrono disagree by {skew}ms: {time}"
    );

    let uuid = body(
        store
            .handle_request("probe", &get("/rand"))
            .expect("uuid v4 inside the guest (wasi:random)"),
    );
    assert_eq!(uuid.len(), 36, "not a uuid: {uuid}");
}

/// (b2) A store that hosts no apps mounts no world (there is no project
/// directory to mount), so a guest that reaches `std::fs` still fails closed
/// instead of receiving an unconfined filesystem: the selective linker only
/// registers `wasi:filesystem` when a world was actually preopened.
#[test]
fn a_store_without_a_world_keeps_the_filesystem_closed() {
    let path = fixture("tests/fixtures/probe-fs/target/wasm32-wasip2/debug/probe_fs.wasm");
    if !path.exists() {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-fs/Cargo.toml"
        );
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("test runtime");
    let store = rt.block_on(store_with_worlds_opt(None));
    let err = store
        .swap_load_with_name(&path, "no-world")
        .expect_err("without a world the filesystem must stay unlinked");
    assert!(
        format!("{err:?}").contains("wasi:filesystem/types"),
        "unexpected error: {err:?}"
    );
}

/// (c) `std::fs` is mounted per app at `/world`, and every path the guest
/// opens is confined to it: the file lands under `plugins/<app>/`, `..`
/// and a sibling app's world are refused, and an absolute host path does not
/// resolve. This is the "full access to its world, nothing outside it" test.
#[test]
fn the_world_is_mounted_read_write_and_confined() {
    let path = fixture("tests/fixtures/probe-fs/target/wasm32-wasip2/debug/probe_fs.wasm");
    if !path.exists() {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-fs/Cargo.toml"
        );
        return;
    }
    let worlds = std::env::temp_dir().join(format!(
        "ferris-worlds-test-{}-{}",
        std::process::id(),
        "mount"
    ));
    let _ = std::fs::remove_dir_all(&worlds);
    std::fs::create_dir_all(&worlds).expect("worlds root");

    // A guest filesystem call is synchronous: run the whole test outside a
    // Tokio context, like the server's blocking thread does, so wasmtime-wasi
    // uses its own fallback runtime instead of nesting one.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("test runtime");
    let store = rt.block_on(store_with_worlds(worlds.clone()));
    store
        .swap_load_with_name(&path, "probe-world")
        .expect("a granted guest must instantiate with wasi:filesystem linked");

    let written = body(
        store
            .handle_request("probe-world", &get("/write"))
            .expect("writing inside the world"),
    );
    // WASI has no process-wide cwd: the guest addresses its world with the
    // absolute guest path the host mounts (`/world/...`), and relative paths
    // are not part of the contract.
    assert_eq!(written, "16", "`inside the world` is 16 bytes");
    let on_disk = worlds.join("probe-world/data/probe.txt");
    assert_eq!(
        std::fs::read_to_string(&on_disk).expect("the host sees the file the guest wrote"),
        "inside the world"
    );

    let escape = body(
        store
            .handle_request("probe-world", &get("/escape"))
            .expect("`..` is an ordinary failed open, not a trap"),
    );
    assert!(
        escape.starts_with("refused:"),
        "`..` escaped the world: {escape}"
    );

    let sibling = body(
        store
            .handle_request("probe-world", &get("/sibling"))
            .expect("a sibling world is just a missing path"),
    );
    assert!(
        sibling.starts_with("refused:"),
        "a sibling app's world is reachable: {sibling}"
    );

    let absolute = store.handle_request("probe-world", &get("/fs"));
    assert!(
        absolute.is_err() || !body(absolute.unwrap()).starts_with("ESCAPED"),
        "an absolute host path resolved"
    );

    let _ = std::fs::remove_dir_all(&worlds);
}

/// (c3) A symlinked world root is refused before the mount: `preopened_dir`
/// would happily follow it to `/` or another app's directory.
#[tokio::test]
async fn a_symlinked_world_root_is_refused() {
    let path = fixture("tests/fixtures/probe-fs/target/wasm32-wasip2/debug/probe_fs.wasm");
    if !path.exists() {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-fs/Cargo.toml"
        );
        return;
    }
    let worlds = std::env::temp_dir().join(format!(
        "ferris-worlds-test-{}-{}",
        std::process::id(),
        "symlink"
    ));
    let target = worlds.with_extension("target");
    let _ = std::fs::remove_dir_all(&worlds);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&worlds).expect("worlds root");
    std::fs::create_dir_all(&target).expect("target");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, worlds.join("probe-link")).expect("symlink");

    let store = store_with_worlds(worlds.clone()).await;
    let err = store
        .swap_load_with_name(&path, "probe-link")
        .expect_err("a symlinked world root must not be mounted");
    assert!(
        format!("{err:?}").contains("symlinked world root"),
        "unexpected error: {err:?}"
    );

    let _ = std::fs::remove_dir_all(&worlds);
    let _ = std::fs::remove_dir_all(&target);
}

/// (d) The context half of the surface: `HostState::new` never calls
/// `inherit_env`/`inherit_args` (verified against `wasmtime-wasi-49` builder
/// defaults: "no env vars, no arguments"), so the guest sees an empty
/// environment even though `wasi:cli/environment` is linked; and stdin is
/// deliberately not inherited, so a read hits EOF instead of stealing the
/// operator's input. The strong form feeds the test process a decoy:
/// `printf leak | cargo test -p ferris-wasm --test selective_wasi`.
#[tokio::test]
async fn guest_env_and_stdin_are_isolated() {
    let path = fixture("tests/fixtures/probe-ok/target/wasm32-wasip2/debug/probe_ok.wasm");
    if !path.exists() {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml"
        );
        return;
    }
    // Precondition: the host process really has a PATH worth leaking,
    // otherwise the guest-side assertion below would pass vacuously.
    assert!(
        std::env::var("PATH").is_ok(),
        "test host has no PATH; the /env assertion proves nothing"
    );
    let store = store().await;
    store
        .swap_load_with_name(&path, "probe")
        .expect("probe-ok must instantiate");

    let env = body(
        store
            .handle_request("probe", &get("/env"))
            .expect("std::env::var inside the guest"),
    );
    assert_eq!(env, "", "guest can read the host environment: {env}");

    let stdin = body(
        store
            .handle_request("probe", &get("/stdin"))
            .expect("stdin read inside the guest"),
    );
    assert_eq!(stdin, "", "guest inherited host stdin: {stdin:?}");
}
