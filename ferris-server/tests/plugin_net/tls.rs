//! The app's own TLS listener: an `https` route and a `wss` upgrade served with
//! the app's certificate.

use std::io;
use std::sync::Arc;

use ferris_server::plugin_net::tls_routes;
use ferris_server::router::DeclaredRouteState;
use ferris_wasm::descriptor::{NetworkSpec, RouteKind, RouteSpec};
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

use super::declared::ws_route;
use super::support::*;

/// A `tls-server` route is served on its own listener with the app's
/// certificate, and the request reaches the guest through the normal dispatch.
#[tokio::test]
async fn a_tls_route_is_served_with_the_apps_certificate() {
    let Some(store) = store().await else { return };
    let record = TlsServer {
        cert: TlsMaterial::Pem(fixture_text("server.crt")),
        key: TlsMaterial::Pem(fixture_text("server.key")),
        ca: None,
        client_auth: ferris_wasm::ClientAuth::None,
        // Only `localhost` is answered: an unlisted name is a handshake failure.
        sni: vec!["localhost".to_string()],
        alpn: vec!["http/1.1".to_string()],
        min_version: ferris_wasm::TlsVersion::Tls12,
    };
    let declarations = vec![(
        "probe".to_string(),
        NetworkSpec {
            capabilities: Vec::new(),
            routes: vec![RouteSpec {
                id: "secure".to_string(),
                method: "GET".to_string(),
                path: "/secure".to_string(),
                kind: RouteKind::Http,
                auth: None,
                body_limit: None,
                tls_server: Some(record),
                listener: None,
            }],
            auth_mode: ferris_wasm::RouteAuth::None,
            http_policy: None,
            tls_client: None,
            tls_server: None,
        },
    )];

    // Reserve a free port, then let the listener take it.
    let probe = TcpListener::bind("127.0.0.1:0").await.expect("probe port");
    let port = probe.local_addr().expect("address").port();
    drop(probe);

    let plans = tls_routes::plan(&store, &declarations, |_| Some(port))
        .await
        .expect("the fixture material plans");
    assert_eq!(plans.len(), 1);
    let state = DeclaredRouteState {
        plugins: Some(Arc::clone(&store)),
        auth: auth(),
    };
    let handle = tls_routes::spawn(plans.into_iter().next().unwrap(), state, &declarations)
        .await
        .expect("the listener binds");

    // Give the accept loop a moment to start before the first handshake; the
    // socket is already bound, so this is a scheduling yield, not a sleep-based
    // synchronisation of the assertion.
    tokio::task::yield_now().await;

    let body = tls_get(
        port,
        "localhost",
        b"GET /secure/echo HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await
    .expect("the TLS request completes");
    assert!(
        body.contains("GET||-|"),
        "the guest answered through the TLS listener: {body}"
    );

    // A name the app does not serve is refused during the handshake.
    let err = tls_get(
        port,
        "other.test",
        b"GET /secure/echo HTTP/1.1\r\nHost: other.test\r\nConnection: close\r\n\r\n",
    )
    .await
    .expect_err("an unlisted SNI name must not be served");
    assert!(
        format!("{err:?}").to_lowercase().contains("certificate")
            || format!("{err:?}").to_lowercase().contains("alert")
            || format!("{err:?}").to_lowercase().contains("handshake"),
        "unexpected failure shape: {err:?}"
    );

    handle.abort();
}

/// One HTTPS request with a client that trusts the fixture CA, returning the raw
/// response text.
async fn tls_get(port: u16, sni: &str, request: &[u8]) -> io::Result<String> {
    let mut roots = rustls::RootCertStore::empty();
    let ca = std::fs::read(fixture("ca.crt"))?;
    let mut reader = io::BufReader::new(ca.as_slice());
    for cert in rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()? {
        roots.add(cert).ok();
    }
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let name = ServerName::try_from(sni.to_string()).map_err(io::Error::other)?;
    let tcp = TcpStream::connect(("127.0.0.1", port)).await?;
    let mut stream = connector
        .connect(name, tcp)
        .await
        .map_err(io::Error::other)?;
    stream.write_all(request).await?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    Ok(String::from_utf8_lossy(&response).into_owned())
}

/// The handshake fixture list is read as DER by the test helper; this pins that
/// the fixture files are the ones the README documents.
#[test]
fn the_tls_fixtures_are_present() {
    for name in [
        "ca.crt",
        "server.crt",
        "server.key",
        "client.crt",
        "client.key",
    ] {
        assert!(fixture(name).exists(), "missing fixture {name}");
    }
    let certs: Vec<CertificateDer<'static>> = {
        let pem = std::fs::read(fixture("server.crt")).expect("server cert");
        let mut reader = io::BufReader::new(pem.as_slice());
        rustls_pemfile::certs(&mut reader)
            .collect::<Result<Vec<_>, _>>()
            .expect("valid PEM")
    };
    assert!(!certs.is_empty());
}

/// A `%stream` route that also declares `tls-server` is `wss`: it is served on
/// the app's dedicated TLS listener, with the app's certificate, as a WebSocket
/// upgrade.
#[tokio::test]
async fn a_wss_stream_route_is_served_on_the_apps_tls_listener() {
    let Some(store) = store().await else { return };
    let record = TlsServer {
        cert: TlsMaterial::Pem(fixture_text("server.crt")),
        key: TlsMaterial::Pem(fixture_text("server.key")),
        ca: None,
        client_auth: ferris_wasm::ClientAuth::None,
        sni: vec!["localhost".to_string()],
        alpn: Vec::new(),
        min_version: ferris_wasm::TlsVersion::Tls12,
    };
    let mut route = ws_route("secure", "/secure-live", None);
    route.tls_server = Some(record);
    let declarations = vec![(
        "probe".to_string(),
        NetworkSpec {
            capabilities: Vec::new(),
            routes: vec![route],
            auth_mode: RouteAuth::None,
            http_policy: None,
            tls_client: None,
            tls_server: None,
        },
    )];

    // Reserve a free port, then let the listener take it.
    let probe = TcpListener::bind("127.0.0.1:0").await.expect("probe port");
    let port = probe.local_addr().expect("address").port();
    drop(probe);

    let plans = tls_routes::plan(&store, &declarations, |_| Some(port))
        .await
        .expect("the fixture material plans");
    assert_eq!(plans.len(), 1, "a wss route needs the app's TLS listener");
    let state = DeclaredRouteState {
        plugins: Some(Arc::clone(&store)),
        auth: auth(),
    };
    let handle = tls_routes::spawn(plans.into_iter().next().unwrap(), state, &declarations)
        .await
        .expect("the TLS listener binds");
    tokio::task::yield_now().await;

    let config = client_config(&fixture_text("ca.crt"));
    let name = ServerName::try_from("localhost".to_string()).expect("server name");
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.expect("dial");
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .expect("the fixture CA verifies the app's certificate");
    let (mut ws, response) =
        tokio_tungstenite::client_async(format!("wss://localhost:{port}/secure-live"), tls)
            .await
            .expect("the wss upgrade completes");
    assert_eq!(response.status().as_u16(), 101);

    ws.send(Message::text("secure ping"))
        .await
        .expect("frame sent");
    let reply = tokio::time::timeout(TIMEOUT, ws.next())
        .await
        .expect("the reply arrives in time")
        .expect("a frame arrives")
        .expect("the frame is valid");
    assert_eq!(reply, Message::binary(b"secure ping".to_vec()));
    handle.abort();
}

/// A rustls client that trusts exactly the fixture CA.
fn client_config(ca_pem: &str) -> rustls::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    let mut reader = io::BufReader::new(ca_pem.as_bytes());
    for cert in rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .expect("valid PEM")
    {
        roots.add(cert).ok();
    }
    rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth()
}
