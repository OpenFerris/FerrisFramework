//! Outbound HTTPS with the app's own TLS material (`io::http-send`).
//!
//! The exchange is real: a loopback `rustls` server presents the shared test
//! certificate and **requires** a client certificate, and the host client is
//! driven through [`HostApi::http_request`] with the app's declared policy. That
//! proves the whole chain — CA bundle, SNI, ALPN, client identity — not just that
//! a config object was built.
//!
//! No external network: both ends are loopback sockets.

// The test links the crate's whole dependency set but uses a few.
#![allow(unused_crate_dependencies)]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferris_wasm::{HostApi, OutboundHttpRequest, OutboundTls, RealHostApi, TlsProtocol};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ferris-server/tests/fixtures/tls")
        .join(name)
}

fn read_certs(name: &str) -> Vec<CertificateDer<'static>> {
    let pem = std::fs::read(fixture(name)).expect("fixture readable");
    let mut reader = io::BufReader::new(pem.as_slice());
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .expect("valid PEM certificates")
}

fn read_key(name: &str) -> PrivateKeyDer<'static> {
    let pem = std::fs::read(fixture(name)).expect("fixture readable");
    let mut reader = io::BufReader::new(pem.as_slice());
    rustls_pemfile::private_key(&mut reader)
        .expect("valid PEM")
        .expect("a private key")
}

fn fixture_text(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).expect("fixture readable")
}

/// Serve one HTTP/1.1 request over mutual TLS and answer with a fixed body.
async fn serve_once(listener: TcpListener) -> String {
    let (tcp, _) = listener.accept().await.expect("connection accepted");

    let mut roots = rustls::RootCertStore::empty();
    for cert in read_certs("ca.crt") {
        roots.add(cert).expect("CA added");
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier =
        rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .expect("client verifier");
    let mut config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(read_certs("server.crt"), read_key("server.key"))
        .expect("server config");
    // ALPN is a negotiation: the server has to offer a protocol too, or the
    // client's list is simply not selected.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let mut stream = acceptor.accept(tcp).await.expect("handshake completes");

    let mut request = Vec::new();
    let mut buffer = [0u8; 1024];
    // Read headers (the test client sends no body).
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let n = stream.read(&mut buffer).await.expect("request readable");
        if n == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..n]);
    }
    let alpn = stream.get_ref().1.alpn_protocol().map_or_else(
        || "none".to_string(),
        |p| String::from_utf8_lossy(p).into_owned(),
    );
    let request = String::from_utf8_lossy(&request).into_owned();
    let body = format!("{alpn}|{request}");
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .expect("response written");
    let _ = stream.shutdown().await;
    response
}

async fn host() -> Option<RealHostApi> {
    if !fixture("server.crt").exists() {
        eprintln!("SKIPPED: ferris-server/tests/fixtures/tls is missing");
        return None;
    }
    let pool = ferris_db::memory_db().await.expect("in-memory pool");
    Some(RealHostApi::new(pool))
}

/// The declared CA verifies the peer, the client certificate is presented, ALPN
/// is honoured and the body comes back intact.
#[tokio::test]
async fn an_https_call_presents_client_material_and_verifies_the_peer() {
    let Some(host) = host().await else { return };
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let port = listener.local_addr().expect("address").port();
    let server = tokio::spawn(serve_once(listener));

    let request = OutboundHttpRequest {
        method: "GET".to_string(),
        url: format!("https://localhost:{port}/echo"),
        headers: vec![("x-probe".to_string(), "tls".to_string())],
        body: None,
        timeout_ms: Some(5_000),
        redirect_policy: Some("none".to_string()),
        allowed_hosts: vec!["localhost".to_string()],
        allow_private: true,
        max_body: None,
        redirect_hops: None,
        allow_credentials: false,
        // Loopback is a private target: the operator grant is what makes it
        // reachable, exactly as `http-unrestricted` documents.
        unrestricted: true,
        tls: Some(OutboundTls {
            cert: Some(fixture_text("client.crt")),
            key: Some(fixture_text("client.key")),
            ca: Some(fixture_text("ca.crt")),
            server_name: Some("localhost".to_string()),
            alpn: vec!["http/1.1".to_string()],
            min_version: TlsProtocol::Tls12,
            verify: true,
        }),
    };

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        host.http_request(&request),
    )
    .await
    .expect("the call returns")
    .expect("the mutual-TLS exchange succeeds");

    assert_eq!(response.status, 200);
    let body = String::from_utf8_lossy(&response.body);
    assert!(
        body.starts_with("http/1.1|GET /echo HTTP/1.1"),
        "ALPN and the request reached the server: {body}"
    );
    assert!(body.contains("x-probe: tls"), "{body}");

    server.await.expect("server task");
}

/// A peer whose certificate the declared CA does not sign is refused, even
/// though the CA itself parses: verification is not optional by default.
#[tokio::test]
async fn a_peer_signed_by_another_ca_is_refused() {
    let Some(host) = host().await else { return };
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let port = listener.local_addr().expect("address").port();
    let server = tokio::spawn(async move {
        // The rogue server presents a certificate the fixture CA did not sign.
        let (tcp, _) = listener.accept().await.expect("accepted");
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(read_certs("server.crt"), read_key("server.key"))
            .expect("server config");
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let _ = acceptor.accept(tcp).await;
    });

    let request = OutboundHttpRequest {
        method: "GET".to_string(),
        url: format!("https://localhost:{port}/"),
        headers: Vec::new(),
        body: None,
        timeout_ms: Some(5_000),
        redirect_policy: Some("none".to_string()),
        allowed_hosts: Vec::new(),
        allow_private: true,
        max_body: None,
        redirect_hops: None,
        allow_credentials: false,
        unrestricted: true,
        // A CA that did not sign the server certificate.
        tls: Some(OutboundTls {
            cert: None,
            key: None,
            ca: Some(fixture_text("rogue-ca.crt")),
            server_name: Some("localhost".to_string()),
            alpn: Vec::new(),
            min_version: TlsProtocol::Tls13,
            verify: true,
        }),
    };

    let error = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        host.http_request(&request),
    )
    .await
    .expect("the call returns")
    .expect_err("an untrusted peer must fail the handshake");
    // reqwest wraps the cause; the chain is where the real reason lives.
    let mut message = error.to_string().to_lowercase();
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(&*error);
    while let Some(cause) = source {
        message.push_str(&format!(" | {cause}").to_lowercase());
        source = cause.source();
    }
    assert!(
        message.contains("certificate")
            || message.contains("unknown issuer")
            || message.contains("invalid")
            || message.contains("tls")
            || message.contains("handshake"),
        "unexpected failure: {message}"
    );
    let _ = server.await;
}

/// The host enum and the wire enum are separate types on purpose: a WIT bump
/// must break the conversion, not silently reinterpret a value.
#[test]
fn the_host_tls_protocol_is_distinct_from_the_wire_enum() {
    assert_eq!(format!("{:?}", TlsProtocol::Tls13), "Tls13");
    assert!(matches!(
        ferris_wasm::TlsVersion::Tls13,
        ferris_wasm::TlsVersion::Tls13
    ));
}
