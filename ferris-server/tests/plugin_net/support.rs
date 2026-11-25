//! Shared fixtures: the guest, the auth state and the loopback helpers.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ferris_auth::{AuthState, SessionConfig};
use ferris_wasm::{
    ListenerTransport, OutboundPolicy, OutboundStreamSpec, PluginStore, RuntimeConfig,
    SharedRuntime,
};

pub use ferris_server::plugin_net::ServerStreams;
pub use ferris_wasm::{
    RouteAuth, StreamListen, StreamTransport, TlsClient, TlsMaterial, TlsServer,
};

/// Generous enough for wasmtime instantiation on a cold cache, short enough that
/// a hung handshake fails the test instead of the suite.
pub const TIMEOUT: Duration = Duration::from_secs(20);

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tls")
        .join(name)
}

pub fn fixture_text(name: &str) -> String {
    std::fs::read_to_string(fixture(name))
        .unwrap_or_else(|e| panic!("reading {} failed: {e}", fixture(name).display()))
}

pub fn probe_ok() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ferris-wasm/tests/fixtures/probe-ok/target/wasm32-wasip2/debug/probe_ok.wasm")
}

pub async fn store() -> Option<Arc<PluginStore>> {
    let path = probe_ok();
    if !path.exists() {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 \
             --manifest-path ferris-wasm/tests/fixtures/probe-ok/Cargo.toml"
        );
        return None;
    }
    let runtime = Arc::new(
        SharedRuntime::new(RuntimeConfig {
            enable_epoch_interrupts: false,
            ..Default::default()
        })
        .expect("SharedRuntime"),
    );
    let pool = ferris_db::memory_db().await.expect("in-memory sqlite pool");
    let store = Arc::new(PluginStore::new(runtime, pool));
    store
        .swap_load_with_name(&path, "probe")
        .expect("probe-ok instantiates");
    store.describe_of("probe").expect("probe describes itself");
    Some(store)
}

/// The stream transport over the shared probe, for the datagram and DNS tests.
pub async fn transport() -> Option<ServerStreams> {
    let store = store().await?;
    Some(ServerStreams::new(store))
}

/// A store whose apps live under `root` (for the process supervisor tests).
pub async fn store_with_worlds(root: PathBuf) -> Arc<PluginStore> {
    let runtime = Arc::new(
        SharedRuntime::new(RuntimeConfig {
            enable_epoch_interrupts: false,
            ..Default::default()
        })
        .expect("SharedRuntime"),
    );
    let pool = ferris_db::memory_db().await.expect("in-memory sqlite pool");
    Arc::new(PluginStore::new(runtime, pool).with_worlds(root))
}

pub fn auth() -> AuthState {
    AuthState::new(SessionConfig::default())
}

pub fn connect_spec(port: u16, allow_private: bool) -> OutboundStreamSpec {
    OutboundStreamSpec {
        transport: ListenerTransport::Tcp,
        uri: format!("tcp://127.0.0.1:{port}"),
        address: None,
        port: None,
        tls: None,
        policy: Some(OutboundPolicy {
            allowed_hosts: vec!["127.0.0.1".to_string()],
            allow_private,
            max_body: None,
            unrestricted: false,
        }),
        max_frame: None,
        idle_timeout_seconds: Some(5),
    }
}
