//! Regression coverage for audit finding H2 / the mTLS plan's Phase 3.
//!
//! Before H2 the WIT `pki` imports existed in the contract but the host answered
//! every call with an "unavailable" error: there was no CA to sign with and no
//! revocation store behind the TLS verifier, so a device could not be enrolled
//! and a revoked device certificate would have kept working until it expired.
//!
//! These tests drive the real [`HostPki`]: they generate an ECDSA CA (the shared
//! fixture CA is RSA, and the `ring` backend refuses to sign with an RSA key —
//! exactly what [`HostPki::load`] must reject), issue a device certificate from
//! a CSR over the same accessor the WASM host uses, and then complete an actual
//! `rustls` handshake. The handshake is the point: a certificate that merely
//! parses proves nothing, while one that satisfies the client-cert verifier and
//! then fails once its fingerprint is revoked proves the whole H2 chain.

// Integration tests link the whole crate's dependency set but use only a
// handful; the workspace's `unused_crate_dependencies` lint would otherwise
// fire once per dependency.
#![allow(unused_crate_dependencies)]

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use ferris_core::{PkiConfig, TlsConfig};
use ferris_server::pki::HostPki;
use ferris_server::tls::sha256_hex;
use ferris_server::{CertificateRevocation, TlsListener, build_server_config};
use ferris_wasm::PkiProvider;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use time::Duration as TimeDuration;
use time::OffsetDateTime;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream as ClientTlsStream;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(15);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tls")
        .join(name)
}

fn fixture_pem(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).expect("fixture is readable")
}

fn certs_from_pem(pem: &str) -> Vec<CertificateDer<'static>> {
    let mut reader = io::BufReader::new(pem.as_bytes());
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .expect("valid PEM certificates")
}

fn key_from_pem(pem: &str) -> PrivateKeyDer<'static> {
    let mut reader = io::BufReader::new(pem.as_bytes());
    rustls_pemfile::private_key(&mut reader)
        .expect("valid PEM private key")
        .expect("a private key is present")
}

/// An ECDSA (P-256) CA generated for the test and written to a temp directory,
/// so [`PkiConfig`] can be pointed at real files the way production does.
struct TestCa {
    cfg: PkiConfig,
    ca_cert_path: PathBuf,
    dir: PathBuf,
}

impl TestCa {
    fn generate() -> Self {
        let mut params = CertificateParams::new(Vec::new()).expect("a CA needs no SAN");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(DnType::CommonName, "Ferris H2 Test CA");
        params.key_usages.push(KeyUsagePurpose::KeyCertSign);
        params.key_usages.push(KeyUsagePurpose::CrlSign);
        params.not_before = OffsetDateTime::now_utc() - TimeDuration::days(1);
        params.not_after = OffsetDateTime::now_utc() + TimeDuration::days(3650);
        let key = KeyPair::generate().expect("ECDSA P-256 key");
        let cert = params.self_signed(&key).expect("self-signed CA");

        let dir = std::env::temp_dir().join(format!("ferris-h2-pki-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp directory");
        let ca_cert_path = dir.join("ca.crt");
        let ca_key_path = dir.join("ca.key");
        std::fs::write(&ca_cert_path, cert.pem()).expect("write the CA certificate");
        std::fs::write(&ca_key_path, key.serialize_pem()).expect("write the CA key");

        Self {
            cfg: PkiConfig {
                ca_cert_path: ca_cert_path.clone(),
                ca_key_path,
                default_ttl_seconds: 3600,
            },
            ca_cert_path,
            dir,
        }
    }
}

impl Drop for TestCa {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A PKCS#10 CSR as a plugin's `enroll` builds it, base64-DER encoded the way
/// the WIT `sign-csr` import expects, plus the private key the certificate must
/// be matched against.
fn device_request(common_name: &str) -> (String, String) {
    let key = KeyPair::generate().expect("device key");
    let mut params = CertificateParams::new(vec!["device.test".to_string()]).expect("valid SAN");
    params
        .distinguished_name
        .push(DnType::CommonName, common_name);
    let csr = params.serialize_request(&key).expect("CSR");
    let csr_b64 = base64::engine::general_purpose::STANDARD.encode(csr.der());
    (csr_b64, key.serialize_pem())
}

/// The server's own cert/key come from the shared fixture; client auth is
/// checked against the freshly generated CA.
fn server_tls(client_ca: &Path) -> TlsConfig {
    TlsConfig {
        cert_path: fixture("server.crt"),
        key_path: fixture("server.key"),
        client_ca_path: Some(client_ca.to_path_buf()),
        require_client_cert: true,
        min_version: None,
    }
}

/// A client that trusts the fixture server chain and presents an issued device
/// certificate.
fn client_config(issued_pem: &str, device_key_pem: &str) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    for cert in certs_from_pem(&fixture_pem("ca.crt")) {
        roots.add(cert).expect("the fixture CA is a valid root");
    }
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .with_root_certificates(roots)
        .with_client_auth_cert(certs_from_pem(issued_pem), key_from_pem(device_key_pem))
        .expect("the issued certificate and key are a usable client identity");
    Arc::new(config)
}

async fn harness(
    tls: &TlsConfig,
    revocation: Option<Arc<dyn CertificateRevocation>>,
) -> (SocketAddr, TlsListener) {
    let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = tcp.local_addr().expect("bound address");
    let config = build_server_config(tls, revocation).expect("server config");
    (addr, TlsListener::new(tcp, config))
}

async fn connect(
    addr: SocketAddr,
    config: Arc<rustls::ClientConfig>,
) -> io::Result<ClientTlsStream<TcpStream>> {
    let tcp = TcpStream::connect(addr).await?;
    let name = ServerName::try_from("localhost").expect("static server name");
    TlsConnector::from(config).connect(name, tcp).await
}

#[tokio::test]
async fn the_host_issues_a_device_certificate_that_completes_an_mtls_handshake() {
    let ca = TestCa::generate();
    let pki = HostPki::load(&ca.cfg, ferris_db::memory_db().await.expect("memory db"))
        .await
        .expect("the generated ECDSA CA loads");

    let (csr_b64, device_key_pem) = device_request("SOLMEC-H2-001");
    let issued_pem = pki
        .pki_sign_csr("test-app", &csr_b64, 0)
        .await
        .expect("the host signs the CSR");
    assert!(
        issued_pem.contains("BEGIN CERTIFICATE"),
        "the issued value must be PEM: {issued_pem}"
    );

    let tls = server_tls(&ca.ca_cert_path);
    let (addr, mut listener) = harness(&tls, None).await;
    let client = tokio::spawn(connect(addr, client_config(&issued_pem, &device_key_pem)));

    let (stream, info) = tokio::time::timeout(TIMEOUT, listener.accept_tls())
        .await
        .expect("handshake within timeout")
        .expect("the issued certificate chains to the CA and satisfies mTLS");
    drop(stream);

    let identity = info.identity.expect("a verified client yields an identity");
    assert_eq!(identity.common_name.as_deref(), Some("SOLMEC-H2-001"));
    assert!(
        identity.issuer.contains("Ferris H2 Test CA"),
        "issuer was {:?}",
        identity.issuer
    );
    let _ = client.await;
}

#[tokio::test]
async fn a_revoked_issued_certificate_is_refused_during_the_handshake() {
    let ca = TestCa::generate();
    let pki = HostPki::load(&ca.cfg, ferris_db::memory_db().await.expect("memory db"))
        .await
        .expect("the generated ECDSA CA loads");

    let (csr_b64, device_key_pem) = device_request("SOLMEC-H2-002");
    let issued_pem = pki
        .pki_sign_csr("test-app", &csr_b64, 0)
        .await
        .expect("the host signs the CSR");
    let fingerprint = sha256_hex(certs_from_pem(&issued_pem)[0].as_ref());

    pki.pki_revoke("test-app", &fingerprint)
        .await
        .expect("the revocation persists");
    assert!(
        pki.pki_is_revoked(&fingerprint).await.expect("query"),
        "a just-revoked fingerprint must read back as revoked"
    );
    // Idempotent: revoking the same certificate twice is a no-op, not an error.
    pki.pki_revoke("test-app", &fingerprint)
        .await
        .expect("revocation is idempotent");

    // The same CA answers the handshake's revocation gate.
    let revocation: Arc<dyn CertificateRevocation> = pki;
    let tls = server_tls(&ca.ca_cert_path);
    let (addr, mut listener) = harness(&tls, Some(revocation)).await;
    let _client = tokio::spawn(connect(addr, client_config(&issued_pem, &device_key_pem)));

    let result = tokio::time::timeout(TIMEOUT, listener.accept_tls())
        .await
        .expect("handshake attempt within timeout");
    assert!(
        result.is_err(),
        "a revoked certificate must be refused during the handshake"
    );
}

#[tokio::test]
async fn only_the_issuing_app_can_revoke_its_certificate() {
    let ca = TestCa::generate();
    let pki = HostPki::load(&ca.cfg, ferris_db::memory_db().await.expect("memory db"))
        .await
        .expect("the generated ECDSA CA loads");

    let (csr_b64, _device_key) = device_request("SOLMEC-H2-SCOPE");
    let issued_pem = pki
        .pki_sign_csr("app-a", &csr_b64, 0)
        .await
        .expect("app-a issues a certificate");
    let fingerprint = sha256_hex(certs_from_pem(&issued_pem)[0].as_ref());

    // Another app cannot revoke it, and the refusal does not reveal whether the
    // fingerprint exists: a foreign and an unknown value get the same error.
    let foreign = pki
        .pki_revoke("app-b", &fingerprint)
        .await
        .expect_err("app-b must not revoke app-a's certificate");
    assert!(
        foreign.to_string().contains("not issued to this app"),
        "{foreign}"
    );
    let unknown = pki
        .pki_revoke("app-b", &"0".repeat(64))
        .await
        .expect_err("an unknown fingerprint is refused the same way");
    assert!(
        unknown.to_string().contains("not issued to this app"),
        "{unknown}"
    );
    assert!(
        !pki.pki_is_revoked(&fingerprint).await.expect("query"),
        "a refused revocation must not mark the certificate revoked"
    );

    // The issuing app can.
    pki.pki_revoke("app-a", &fingerprint)
        .await
        .expect("app-a revokes its own certificate");
    assert!(pki.pki_is_revoked(&fingerprint).await.expect("query"));
}

#[tokio::test]
async fn a_malformed_fingerprint_cannot_be_revoked() {
    let ca = TestCa::generate();
    let pki = HostPki::load(&ca.cfg, ferris_db::memory_db().await.expect("memory db"))
        .await
        .expect("the generated ECDSA CA loads");

    assert!(
        pki.pki_revoke("test-app", "not-a-fingerprint")
            .await
            .is_err(),
        "a non-hex value must be rejected instead of stored"
    );
    let short = "a".repeat(63);
    assert!(
        pki.pki_revoke("test-app", &short).await.is_err(),
        "a wrong-length digest must be rejected"
    );
}

#[tokio::test]
async fn the_registered_provider_serves_the_pki_imports() {
    let ca = TestCa::generate();
    let pki = HostPki::load(&ca.cfg, ferris_db::memory_db().await.expect("memory db"))
        .await
        .expect("the generated ECDSA CA loads");

    // `Server::run` registers the CA once at boot and `RealHostApi` reaches it
    // through this same accessor, so a `Some` here is exactly what turns the WIT
    // `pki` imports from "unavailable" into working ones. This is the only test
    // in this binary that touches the process-global slot, so it is
    // deterministic.
    assert!(
        ferris_wasm::set_pki_provider(pki),
        "the PKI slot must be free"
    );
    let provider = ferris_wasm::pki_provider().expect("a provider is registered");

    let (csr_b64, _device_key) = device_request("SOLMEC-H2-003");
    let issued = provider
        .pki_sign_csr("test-app", &csr_b64, 0)
        .await
        .expect("signed through the registered provider");
    let fingerprint = sha256_hex(certs_from_pem(&issued)[0].as_ref());
    assert!(!provider.pki_is_revoked(&fingerprint).await.expect("query"));
    provider
        .pki_revoke("test-app", &fingerprint)
        .await
        .expect("the provider persists the revocation");
    assert!(provider.pki_is_revoked(&fingerprint).await.expect("query"));

    assert_eq!(
        provider.pki_ca_cert().await.expect("CA certificate"),
        std::fs::read_to_string(&ca.ca_cert_path).expect("read the CA certificate")
    );
}
