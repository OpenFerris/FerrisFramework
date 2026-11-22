//! SRV2 / SRV15 regressions: the advertised origin follows the explicit
//! setting and TLS, and `run_blocking` stays off `block_in_place` unless the
//! runtime is multi-threaded.
use ferris_core::{Config, DatabaseConfig};

use super::super::oauth::public_base_url;
#[cfg(feature = "wasm")]
use super::super::plugins::run_blocking;
use crate::server::ServerSettings;

fn settings(host: &str, port: u16) -> ServerSettings {
    ServerSettings::new(
        host.to_string(),
        port,
        crate::roles::ServerRoles::default(),
        DatabaseConfig::default(),
        Config::default(),
    )
}

#[test]
fn configured_external_url_wins_and_loses_the_trailing_slash() {
    let mut settings = settings("127.0.0.1", 8000);
    settings.config.server.external_url = Some("https://erp.example.com/".to_string());
    assert_eq!(public_base_url(&settings), "https://erp.example.com");
}

#[test]
fn tls_selects_https_for_the_fallback() {
    let mut settings = settings("erp.example.com", 8443);
    settings.config.server.tls = Some(ferris_core::TlsConfig {
        cert_path: "cert.pem".into(),
        key_path: "key.pem".into(),
        ..Default::default()
    });
    assert_eq!(public_base_url(&settings), "https://erp.example.com:8443");
}

#[test]
fn plain_http_is_the_default_fallback() {
    let settings = settings("127.0.0.1", 8000);
    assert_eq!(public_base_url(&settings), "http://127.0.0.1:8000");
}

#[cfg(feature = "wasm")]
#[test]
fn run_blocking_executes_outside_a_runtime() {
    assert_eq!(run_blocking(|| 7), 7);
}

#[cfg(feature = "wasm")]
#[tokio::test(flavor = "current_thread")]
async fn run_blocking_does_not_panic_on_a_current_thread_runtime() {
    // SRV15: `block_in_place` panics on a current-thread runtime, which is
    // exactly the reactor `#[tokio::test]` builds.
    assert_eq!(run_blocking(|| 7), 7);
}
