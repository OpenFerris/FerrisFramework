//! End-to-end coverage of the Phase 1 TLS/mTLS path.
//!
//! Each test drives a real `rustls` handshake through [`TlsListener`], so
//! certificate-chain verification, the client-cert mandate, the revocation
//! gate and the identity extraction are exercised together rather than in
//! isolation. `accept_tls` (not the retrying `Listener::accept`) is used so a
//! rejected handshake is observable as an `Err`.

// Integration tests link the whole crate's dependency set but use only a
// handful; the workspace's `unused_crate_dependencies` lint would otherwise
// fire once per dependency.
#![allow(unused_crate_dependencies)]

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ferris_core::TlsConfig;
use ferris_server::tls::sha256_hex;
use ferris_server::{CertificateRevocation, TlsListener, build_server_config};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream as ClientTlsStream;

const TIMEOUT: Duration = Duration::from_secs(15);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tls")
        .join(name)
}

fn read_certs(name: &str) -> Vec<CertificateDer<'static>> {
    let pem = std::fs::read(fixture(name)).unwrap();
    let mut reader = io::BufReader::new(pem.as_slice());
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn read_key(name: &str) -> PrivateKeyDer<'static> {
    let pem = std::fs::read(fixture(name)).unwrap();
    let mut reader = io::BufReader::new(pem.as_slice());
    rustls_pemfile::private_key(&mut reader).unwrap().unwrap()
}

/// Server config with client auth against the trusted CA.
fn server_tls(require_client_cert: bool) -> TlsConfig {
    TlsConfig {
        cert_path: fixture("server.crt"),
        key_path: fixture("server.key"),
        client_ca_path: Some(fixture("ca.crt")),
        require_client_cert,
        min_version: None,
    }
}

/// Client config trusting `ca`, optionally presenting a client certificate.
fn client_config(client: Option<(&str, &str)>, ca: &str) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    for cert in read_certs(ca) {
        roots.add(cert).unwrap();
    }
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots);
    let config = match client {
        Some((cert, key)) => builder
            .with_client_auth_cert(read_certs(cert), read_key(key))
            .unwrap(),
        None => builder.with_no_client_auth(),
    };
    Arc::new(config)
}

/// Complete a client handshake, keeping the stream alive until the caller drops
/// it (so the server can finish its side).
async fn connect(
    addr: SocketAddr,
    config: Arc<rustls::ClientConfig>,
) -> io::Result<ClientTlsStream<TcpStream>> {
    let tcp = TcpStream::connect(addr).await?;
    let name = ServerName::try_from("localhost").unwrap();
    TlsConnector::from(config).connect(name, tcp).await
}

async fn harness(
    tls: TlsConfig,
    revocation: Option<Arc<dyn CertificateRevocation>>,
) -> (SocketAddr, TlsListener) {
    let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let config = build_server_config(&tls, revocation).unwrap();
    (addr, TlsListener::new(tcp, config))
}

#[tokio::test]
async fn a_valid_client_certificate_is_reported_as_identity() {
    let (addr, mut listener) = harness(server_tls(true), None).await;
    let client = tokio::spawn(connect(
        addr,
        client_config(Some(("client.crt", "client.key")), "ca.crt"),
    ));

    let (stream, info) = tokio::time::timeout(TIMEOUT, listener.accept_tls())
        .await
        .expect("handshake within timeout")
        .expect("a valid client certificate completes the handshake");
    drop(stream);

    let identity = info
        .identity
        .expect("verified client cert yields an identity");
    assert_eq!(identity.common_name.as_deref(), Some("SOLMEC-001"));
    assert!(identity.subject.contains("SOLMEC-001"));
    assert!(identity.issuer.contains("Ferris Test Root CA"));
    assert_eq!(identity.fingerprint.len(), 64);

    // Keep the client stream alive until the server side is done.
    let _ = client.await;
}

#[tokio::test]
async fn an_untrusted_client_certificate_fails_the_handshake() {
    let (addr, mut listener) = harness(server_tls(true), None).await;
    // The client trusts the rogue CA, so it happily presents the rogue cert;
    // the server trusts only the real CA and must refuse it.
    let _client = tokio::spawn(connect(
        addr,
        client_config(
            Some(("rogue-client.crt", "rogue-client.key")),
            "rogue-ca.crt",
        ),
    ));

    let result = tokio::time::timeout(TIMEOUT, listener.accept_tls())
        .await
        .expect("handshake attempt within timeout");
    assert!(result.is_err(), "a cert from an unknown CA must be refused");
}

#[tokio::test]
async fn a_missing_client_certificate_is_refused_when_required() {
    let (addr, mut listener) = harness(server_tls(true), None).await;
    let _client = tokio::spawn(connect(addr, client_config(None, "ca.crt")));

    let result = tokio::time::timeout(TIMEOUT, listener.accept_tls())
        .await
        .expect("handshake attempt within timeout");
    assert!(result.is_err(), "mTLS must reject an anonymous client");
}

#[tokio::test]
async fn an_anonymous_client_is_accepted_when_client_cert_is_optional() {
    let (addr, mut listener) = harness(server_tls(false), None).await;
    let client = tokio::spawn(connect(addr, client_config(None, "ca.crt")));

    let (stream, info) = tokio::time::timeout(TIMEOUT, listener.accept_tls())
        .await
        .expect("handshake within timeout")
        .expect("optional client auth accepts an anonymous client");
    drop(stream);

    assert!(
        info.identity.is_none(),
        "an anonymous client has no identity"
    );
    let _ = client.await;
}

#[derive(Debug)]
struct RevokeOne {
    fingerprint: String,
}

impl CertificateRevocation for RevokeOne {
    fn is_revoked(&self, fingerprint_hex: &str) -> bool {
        fingerprint_hex == self.fingerprint
    }
}

#[tokio::test]
async fn a_revoked_client_certificate_fails_the_handshake() {
    let fingerprint = sha256_hex(read_certs("client.crt")[0].as_ref());
    let revocation: Arc<dyn CertificateRevocation> = Arc::new(RevokeOne { fingerprint });

    let (addr, mut listener) = harness(server_tls(true), Some(revocation)).await;
    let _client = tokio::spawn(connect(
        addr,
        client_config(Some(("client.crt", "client.key")), "ca.crt"),
    ));

    let result = tokio::time::timeout(TIMEOUT, listener.accept_tls())
        .await
        .expect("handshake attempt within timeout");
    assert!(
        result.is_err(),
        "a revoked certificate must be refused during the handshake"
    );
}
