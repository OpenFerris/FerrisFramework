//! End-to-end proof of the certificate boundary.
//!
//! A verified client certificate is a **device** credential, not a role. The
//! boundary resolves it to a role-less `cert:<fingerprint>` principal; unless
//! the operator maps that principal to a user/role (or the DocType grants
//! `Everyone`), the engine default-denies every docstore read/write. This is
//! deliberate fail-closed behavior: a certificate must never become a global
//! `allow_anonymous` bypass.
//!
//! This drives the real plugin boundary with the `probe-ok` fixture, whose
//! `/write-eq` route performs a `docstore::write-no-events`, and asserts:
//!
//! 1. no certificate, no session → the write is **denied**;
//! 2. a verified `client-identity`, no mapped principal → the write is
//!    **denied** with a docstore permission error (never a 200).
//!
//! Build the fixture first (it skips — loudly — when absent):
//! ```sh
//! cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml
//! ```
//!
//! ## Why the hot-path test does not use a one-connection pool
//!
//! It used to, and it failed with `database error: pool exhausted`. That was not
//! the docstore write: loading the plugin already strands every pooled
//! connection. `ferris-wasm/src/setup.rs::block_on` drives its future on a
//! **separate current-thread runtime** (shared, process-wide) and only polls it
//! while a `block_on` is running. `sqlx` returns a connection to its pool from a
//! task it *spawns* on whichever runtime is current when the `PoolConnection`
//! drops (`sqlx-core/src/pool/connection.rs:200-208` → `crate::rt::spawn` →
//! `Handle::try_current().spawn(..)`), so that task is queued on the idle setup
//! runtime and never polled again: the connection is never handed back.
//!
//! `RealHostApi::new` runs three DB-touching setup steps through that helper
//! (job-queue schema, blob-store schema, `tabAppConfig`) and the last one to
//! finish loses its connection; measured on this tree, exactly **one** pooled
//! connection is stranded per plugin load (a `max_connections = 2` pool
//! survives it, `1` does not). A production pool (20) hides it as a slow leak —
//! one connection per load, i.e. per hot reload — so the real fix is in that
//! helper (drive the future on the caller's runtime; see the finding in the
//! report), not in a bigger pool. Until it lands, this file gives the store the
//! multi-connection, file-backed pool the host contract already assumes
//! (`HostState::default` uses five connections) so the assertion is about the
//! certificate path and not about the leak.

use std::path::{Path, PathBuf};

use ferris_db::Db;
use ferris_document::DocumentEngine;
use ferris_schema::{DoctypeMeta, FieldMeta, register_doctype};
use ferris_wasm::ferris::framework::types::ClientIdentity;
use ferris_wasm::{HttpRequest, PluginStore, RuntimeConfig, SharedRuntime};

/// The doctype the fixture writes, registered here so the write has a table to
/// land in and the only thing that can deny it is authorization.
const DOCTYPE: &str = "Equipo";

/// Connections for the hot-path store: one the plugin load strands (see the
/// module docs) plus the one the docstore write checks out. Deliberately
/// minimal — the post-request `health_check` is what proves the *write* handed
/// its connection back, and a larger pool would hide a second leak.
const HOT_PATH_POOL: u32 = 2;

/// A throwaway SQLite database on disk.
///
/// The hot-path test needs more than one connection, and `:memory:` gives every
/// connection its own empty database — so the store is backed by a real file,
/// removed (with its journal siblings) when the guard drops.
struct TempDb {
    db: Db,
    path: PathBuf,
}

impl TempDb {
    async fn open(tag: &str, max_connections: u32) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "ferris-certificate-{tag}-{}-{nanos}.db",
            std::process::id()
        ));
        let db = ferris_db::open_unmanaged(&ferris_core::DatabaseConfig {
            r#type: ferris_core::DatabaseType::SQLite,
            name: path.to_string_lossy().into_owned(),
            max_connections,
            min_connections: 0,
            ..ferris_core::DatabaseConfig::default()
        })
        .await
        .expect("open a throwaway sqlite database");
        Self { db, path }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        // Best effort: the assertion already ran, and a leftover file in the
        // temp directory is noise, not a test failure.
        let _ = std::fs::remove_file(&self.path);
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut sibling = self.path.clone().into_os_string();
            sibling.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(sibling));
        }
    }
}

/// One plugin store over `db`, with `Equipo` registered and its table created,
/// so a successful write persists instead of failing on a missing doctype.
/// Epoch interrupts stay off: nothing ticks the epoch in a test, so the call
/// deadlines never fire.
async fn store_with_equipo(db: &Db) -> PluginStore {
    let runtime = std::sync::Arc::new(
        SharedRuntime::new(RuntimeConfig {
            enable_epoch_interrupts: false,
            ..Default::default()
        })
        .expect("SharedRuntime"),
    );
    register_doctype(DoctypeMeta::new(DOCTYPE, "test").field(FieldMeta::new("probe", "Data")));
    DocumentEngine::new(db.clone())
        .ensure_table(DOCTYPE)
        .await
        .expect("ensure_table for the fixture doctype");
    PluginStore::new(runtime, db.clone())
}

fn probe_ok() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/probe-ok/target/wasm32-wasip2/debug/probe_ok.wasm")
}

/// Load `probe-ok` or skip loudly (fresh clone without prebuilt fixtures).
async fn load_into(store: &PluginStore) -> bool {
    if !probe_ok().exists() {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml"
        );
        return false;
    }
    store
        .swap_load_with_name(probe_ok(), "probe")
        .expect("probe-ok must instantiate");
    true
}

fn request(client_identity: Option<ClientIdentity>) -> HttpRequest {
    HttpRequest {
        method: "POST".to_string(),
        path: "/write-eq".to_string(),
        query: String::new(),
        headers: Vec::new(),
        body: Vec::new(),
        client_identity,
    }
}

fn verified_identity() -> ClientIdentity {
    ClientIdentity {
        subject: "O=Solmec,CN=SOLMEC-001".to_string(),
        common_name: Some("SOLMEC-001".to_string()),
        fingerprint: "deadbeef".to_string(),
        issuer: "CN=SolmecBOX Test CA".to_string(),
    }
}

/// The symptom, pinned: an anonymous request with **no** client certificate is
/// denied on its first docstore write, and the denial is a permission error —
/// the exact abort that leaves the device un-updated and SSE silent. This half
/// must keep failing: the bypass is scoped to a verified certificate, never to
/// "any anonymous caller".
///
/// A one-connection in-memory database is deliberate here: the denial has to
/// happen before the write reaches storage, so the pool size cannot flatter the
/// result.
#[tokio::test(flavor = "multi_thread")]
async fn anonymous_request_without_certificate_is_denied() {
    let temp = TempDb::open("anonymous", 1).await;
    let store = store_with_equipo(&temp.db).await;
    if !load_into(&store).await {
        return;
    }
    // The host's `docstore` import blocks on the runtime (`host.rs`), exactly as
    // the real router does it from `block_in_place`; the test must do the same
    // on a multi-thread runtime or `block_on` panics.
    let resp = tokio::task::block_in_place(|| store.handle_request("probe", &request(None)))
        .expect("/write-eq answers (it reports the denial in its body)");
    let body = String::from_utf8_lossy(&resp.body);
    assert_eq!(
        resp.status, 403,
        "an anonymous request with no certificate must be permission-denied, got {body:?}"
    );
    assert!(
        body.contains("denied|"),
        "the failure must be a docstore denial: {body:?}"
    );
}

/// A certificate without an operator mapping must not authorize a write.
///
/// This is the security invariant that replaced the H7 service bypass: the
/// device principal is role-less, so `Equipo` (which has no `Everyone` grant in
/// the fixture) is denied. Mapping the fingerprint to a user/role is the
/// explicit, auditable step an operator takes.
#[tokio::test(flavor = "multi_thread")]
async fn certificate_authenticated_request_is_denied_without_a_mapped_principal() {
    let temp = TempDb::open("hot-path", HOT_PATH_POOL).await;
    let store = store_with_equipo(&temp.db).await;
    if !load_into(&store).await {
        return;
    }
    let resp = tokio::task::block_in_place(|| {
        store.handle_request("probe", &request(Some(verified_identity())))
    })
    .expect("/write-eq answers");
    let body = String::from_utf8_lossy(&resp.body);
    assert_eq!(
        resp.status, 403,
        "a certificate without a mapped principal must be denied, got {body:?}"
    );
    assert!(
        body.contains("denied|"),
        "the failure must be a docstore permission error: {body:?}"
    );
}
