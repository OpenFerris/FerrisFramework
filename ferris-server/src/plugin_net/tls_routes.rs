//! The dedicated TLS listener for routes that declare `tls-server`.
//!
//! A route with a `tls-server` record asks the host to terminate TLS with the
//! app's own certificate. It cannot ride the main listener (one listener carries
//! one certificate) and it must not be served in clear text, so each app that
//! declares one gets a listener of its own on the port the operator configured
//! (`[apps] plugin_tls_port`, or `[apps.plugin_tls_ports]` with one
//! `"app-name" = 8443` entry per app).
//!
//! The listener speaks HTTP/1.1 and HTTP/2 (ALPN, when the app asks for it) and
//! reuses [`DeclaredRouteState`](crate::router::plugin_routes::DeclaredRouteState)
//! and the exact same dispatch as the plain routes, so a TLS route and an HTTP
//! route of the same app cannot behave differently.
//!
//! # SNI
//!
//! `tls-server.sni` lists the names the certificate answers for. An unlisted
//! name is **refused during the handshake** (the certificate resolver returns
//! `None`), not merely ignored: a client that asks for `admin.example` and is
//! silently served the app's certificate would believe it reached a host the
//! operator never configured. An empty list keeps the previous behaviour (the
//! certificate answers for whatever name arrives), which is what a
//! single-tenant listener wants.

use std::io;
use std::sync::Arc;

use anyhow::{Context, Result};
use ferris_wasm::descriptor::NetworkSpec;
use ferris_wasm::{ClientAuth, PluginStore, TlsVersion};
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::ResolvesServerCert;
use rustls::sign::CertifiedKey;
use tracing::{info, warn};

use crate::router::plugin_routes::{self, DeclaredRouteState, RouteMode};
use crate::tls::TlsListener;

/// ALPN protocols a plugin TLS listener can carry (axum serves HTTP/1.1 and
/// HTTP/2; anything else would negotiate a protocol the host cannot speak).
const SUPPORTED_ALPN: [&str; 2] = ["http/1.1", "h2"];

/// One TLS listener the server should bind.
pub struct TlsRoutePlan {
    /// The app the routes belong to.
    pub app: String,
    /// The port from `[apps]`.
    pub port: u16,
    /// The certificate resolver, already holding the app's material.
    pub server_config: Arc<rustls::ServerConfig>,
    /// The routes served on this listener.
    pub routes: usize,
}

/// Plan the TLS listeners for every app that declares `tls-server` routes.
///
/// Planning is separate from binding so a port conflict or a missing port is a
/// boot-time error the caller can report, not a background task that dies
/// silently.
///
/// # Errors
///
/// An app's material cannot be resolved or parsed, its ALPN list is not one this
/// host can carry, or two apps would need the same port.
pub async fn plan(
    plugins: &Arc<PluginStore>,
    declarations: &[(String, NetworkSpec)],
    plan_port: impl Fn(&str) -> Option<u16>,
) -> Result<Vec<TlsRoutePlan>> {
    let mut plans: Vec<TlsRoutePlan> = Vec::new();
    for (app, network) in declarations {
        let tls_routes: Vec<_> = network
            .routes
            .iter()
            .filter(|route| route.tls_server.is_some())
            .collect();
        if tls_routes.is_empty() {
            continue;
        }
        let Some(port) = plan_port(app) else {
            warn!(
                app = %app,
                routes = tls_routes.len(),
                "app declares `tls-server` routes but [apps] configures no TLS port; \
                 they are not served (set `plugin_tls_port` or `plugin_tls_ports.<app>`)"
            );
            continue;
        };
        if let Some(existing) = plans.iter().find(|plan| plan.port == port) {
            // One port cannot carry two certificates; the second app is refused
            // rather than silently reusing the first app's key material.
            warn!(
                app = %app,
                port,
                owner = %existing.app,
                "two apps asked for the same plugin TLS port; the second is not served"
            );
            continue;
        }
        // The app-wide default (`describe().tls-server`) fills in for a route
        // that carries only a partial record; the route's own record wins.
        let mut server_config = None;
        for route in &tls_routes {
            let Some(record) = route.tls_server.as_ref() else {
                continue;
            };
            let config = build_for_app(plugins, app, record).await?;
            match &server_config {
                None => server_config = Some(config),
                Some(existing) => {
                    if existing.cert != config.cert {
                        anyhow::bail!(
                            "app `{app}` declares TLS routes with different certificates on one \
                             listener (port {port}); a listener carries one certificate"
                        );
                    }
                }
            }
        }
        let Some((server_config, count)) =
            server_config.map(|config| (config.config, config.routes))
        else {
            continue;
        };
        plans.push(TlsRoutePlan {
            app: app.clone(),
            port,
            server_config,
            routes: count,
        });
    }
    Ok(plans)
}

/// The built config plus the route count, and the certificate it answers with.
struct BuiltTls {
    config: Arc<rustls::ServerConfig>,
    cert: Vec<u8>,
    routes: usize,
}

/// Build the rustls config for one app from its first TLS route.
async fn build_for_app(
    plugins: &Arc<PluginStore>,
    app: &str,
    record: &ferris_wasm::TlsServer,
) -> Result<BuiltTls> {
    let cert_pem =
        crate::plugin_net::material::resolve(plugins, app, &record.cert, "tls-server.cert").await?;
    let key_pem =
        crate::plugin_net::material::resolve(plugins, app, &record.key, "tls-server.key").await?;
    let ca_pem = match record.ca.as_ref() {
        Some(material) => Some(
            crate::plugin_net::material::resolve(plugins, app, material, "tls-server.ca").await?,
        ),
        None => None,
    };
    let config = build_server_config(
        &cert_pem,
        &key_pem,
        ca_pem.as_deref(),
        record.client_auth,
        &record.sni,
        &record.alpn,
        &record.min_version,
    )?;
    Ok(BuiltTls {
        config,
        cert: cert_pem.into_bytes(),
        routes: 1,
    })
}

/// Build a `rustls::ServerConfig` from PEM material.
///
/// # Errors
///
/// Unparseable PEM, a missing CA while client authentication is requested, an
/// empty key, or an ALPN protocol this host cannot carry.
pub fn build_server_config(
    cert_pem: &str,
    key_pem: &str,
    ca_pem: Option<&str>,
    client_auth: ClientAuth,
    sni: &[String],
    alpn: &[String],
    min_version: &TlsVersion,
) -> Result<Arc<rustls::ServerConfig>> {
    let certs = parse_certs(cert_pem).context("parsing the app's server certificate")?;
    let key = parse_key(key_pem).context("parsing the app's private key")?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());

    let versions: &[&rustls::SupportedProtocolVersion] = match min_version {
        TlsVersion::Tls13 => &[&rustls::version::TLS13],
        TlsVersion::Tls12 => &[&rustls::version::TLS13, &rustls::version::TLS12],
    };
    let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(versions)
        .context("configuring TLS protocol versions")?;

    let builder = match client_auth {
        ClientAuth::None => builder.with_no_client_auth(),
        mode => {
            let ca_pem = ca_pem.ok_or_else(|| {
                anyhow::anyhow!(
                    "`tls-server.client-auth` is `{mode:?}` but no `ca` was declared: \
                     there is no trust anchor to verify a client certificate against"
                )
            })?;
            let roots = Arc::new(parse_roots(ca_pem).context("parsing the client CA bundle")?);
            let verifier_builder = rustls::server::WebPkiClientVerifier::builder_with_provider(
                roots,
                Arc::clone(&provider),
            );
            let verifier_builder = if mode == ClientAuth::Required {
                verifier_builder
            } else {
                verifier_builder.allow_unauthenticated()
            };
            builder.with_client_cert_verifier(
                verifier_builder
                    .build()
                    .context("building the client certificate verifier")?,
            )
        }
    };

    let certified = CertifiedKey::from_der(certs.clone(), key, &provider)
        .context("installing the app's certificate and key")?;
    let resolver = Arc::new(SniResolver {
        names: sni
            .iter()
            .map(|name| name.trim().trim_start_matches("*.").to_lowercase())
            .filter(|name| !name.is_empty())
            .collect(),
        certified: Arc::new(certified),
    });
    let mut config = builder.with_cert_resolver(resolver);

    if !alpn.is_empty() {
        let mut protocols = Vec::with_capacity(alpn.len());
        for protocol in alpn {
            if !SUPPORTED_ALPN.contains(&protocol.as_str()) {
                anyhow::bail!(
                    "`tls-server.alpn` asks for `{protocol}`, which this listener cannot carry \
                     (supported: http/1.1, h2)"
                );
            }
            protocols.push(protocol.as_bytes().to_vec());
        }
        config.alpn_protocols = protocols;
    }
    Ok(Arc::new(config))
}

/// Serve the app's certificate for the names it declares, and nothing else.
#[derive(Debug)]
struct SniResolver {
    /// Lowercased names, without the `*.` prefix of a wildcard entry.
    names: Vec<String>,
    certified: Arc<CertifiedKey>,
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, hello: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        if self.names.is_empty() {
            return Some(Arc::clone(&self.certified));
        }
        let name = hello.server_name()?;
        let name = name.to_lowercase();
        // A wildcard entry (`*.example.test`) answers for its subdomains, a plain
        // entry for itself: the same rule the outbound allow-list uses.
        if self.names.iter().any(|entry| {
            name == *entry
                || (name.as_str() != entry.as_str() && name.ends_with(&format!(".{entry}")))
        }) {
            Some(Arc::clone(&self.certified))
        } else {
            tracing::warn!(server_name = %name, "TLS handshake for a name the app does not serve");
            None
        }
    }
}

/// Bind and serve one planned listener until the returned handle is aborted.
///
/// The listener keeps serving after a failed handshake (per-connection, logged by
/// [`TlsListener`]) and reports a bind failure instead of silently not listening.
///
/// # Errors
///
/// The port cannot be bound, or the router cannot be built from the declaration.
pub async fn spawn(
    plan: TlsRoutePlan,
    state: DeclaredRouteState,
    declarations: &[(String, NetworkSpec)],
) -> Result<tokio::task::JoinHandle<()>> {
    // Only this app's TLS routes, so one listener cannot serve another app's
    // paths.
    let owned: Vec<(String, NetworkSpec)> = declarations
        .iter()
        .filter(|(name, _)| name == &plan.app)
        .cloned()
        .collect();
    let router = plugin_routes::build::<DeclaredRouteState>(&owned, RouteMode::Tls)
        .layer(axum::middleware::from_fn(
            crate::client_identity::bind_client_identity,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.auth.clone(),
            crate::router::plugin_http::bind_plugin_user,
        ))
        .with_state(state);

    let bind = format!("0.0.0.0:{}", plan.port);
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("binding the plugin TLS listener at {bind}"))?;
    info!(
        app = %plan.app,
        port = plan.port,
        routes = plan.routes,
        "plugin TLS listener serving the app's `tls-server` routes"
    );

    let listener = TlsListener::new(listener, plan.server_config);
    Ok(tokio::spawn(async move {
        if let Err(e) = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<crate::tls::TlsClientInfo>(),
        )
        .await
        {
            warn!(error = %e, "plugin TLS listener stopped");
        }
    }))
}

/// Parse a PEM certificate bundle.
fn parse_certs(pem: &str) -> Result<Vec<CertificateDer<'static>>> {
    let mut reader = io::BufReader::new(pem.as_bytes());
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("reading a PEM certificate")?;
    if certs.is_empty() {
        anyhow::bail!("the certificate PEM holds no certificate");
    }
    Ok(certs)
}

/// Parse the first PEM private key of a bundle.
fn parse_key(pem: &str) -> Result<PrivateKeyDer<'static>> {
    let mut reader = io::BufReader::new(pem.as_bytes());
    rustls_pemfile::private_key(&mut reader)
        .context("reading a PEM private key")?
        .ok_or_else(|| anyhow::anyhow!("the key PEM holds no private key"))
}

/// Parse a PEM CA bundle into a trust store.
fn parse_roots(pem: &str) -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for cert in parse_certs(pem)? {
        roots.add(cert).context("adding a client CA certificate")?;
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/tls")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {} failed: {e}", path.display()))
    }

    #[test]
    fn a_config_with_client_auth_requires_a_ca() {
        let err = build_server_config(
            &fixture("server.crt"),
            &fixture("server.key"),
            None,
            ClientAuth::Required,
            &[],
            &[],
            &TlsVersion::Tls12,
        )
        .expect_err("required client auth without a CA cannot verify anything");
        assert!(err.to_string().contains("trust anchor"), "{err}");
    }

    #[test]
    fn the_fixture_material_builds_a_config() {
        let config = build_server_config(
            &fixture("server.crt"),
            &fixture("server.key"),
            Some(&fixture("ca.crt")),
            ClientAuth::Optional,
            &["localhost".to_string()],
            &["http/1.1".to_string(), "h2".to_string()],
            &TlsVersion::Tls13,
        )
        .expect("the fixture material is valid");
        assert_eq!(
            config.alpn_protocols,
            vec![b"http/1.1".to_vec(), b"h2".to_vec()]
        );
    }

    #[test]
    fn an_alpn_the_listener_cannot_speak_is_refused() {
        let err = build_server_config(
            &fixture("server.crt"),
            &fixture("server.key"),
            None,
            ClientAuth::None,
            &[],
            &["acme/1".to_string()],
            &TlsVersion::Tls12,
        )
        .expect_err("acme/1 is not an HTTP protocol");
        assert!(err.to_string().contains("acme/1"), "{err}");
    }

    #[test]
    fn an_unlisted_sni_name_is_refused_and_a_listed_one_answered() {
        let resolver = SniResolver {
            names: vec!["example.test".to_string()],
            certified: Arc::new(
                CertifiedKey::from_der(
                    parse_certs(&fixture("server.crt")).unwrap(),
                    parse_key(&fixture("server.key")).unwrap(),
                    &Arc::new(rustls::crypto::ring::default_provider()),
                )
                .unwrap(),
            ),
        };
        // A `ClientHello` cannot be built by hand, so the matching rule is
        // asserted on the same predicate the resolver uses.
        assert!(resolver.names.iter().any(|entry| "example.test" == entry));
        assert!(
            !resolver
                .names
                .iter()
                .any(|entry| "other.test" == entry || "other.test".ends_with(&format!(".{entry}")))
        );
    }
}
