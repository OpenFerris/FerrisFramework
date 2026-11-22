//! Outbound TLS built from an app's declared `tls-client` policy.
//!
//! The guest never performs a handshake: it declares the material and the
//! policy, and the host builds a `rustls::ClientConfig` from them. This module
//! is the single place that turns a [`OutboundTls`] into that config, so the
//! HTTP client (`host_impl::http`) and the stream transport (the server) cannot
//! disagree about what `verify: false` or `min-version: tls13` means.
//!
//! The policy is already capability-checked by the bridge (`tls-client`, plus
//! `http-unrestricted` for `verify: false`); this module only reports
//! structural problems — unparseable PEM, a key without a certificate, an ALPN
//! the host cannot speak — as typed errors instead of silently ignoring them.
//! Silence here would mean a plugin asking for client-certificate
//! authentication quietly getting an anonymous connection.

use std::sync::Arc;

use anyhow::{Context, Result};
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};

use crate::host::{OutboundTls, TlsProtocol};

/// ALPN protocols the host can actually carry. A guest may name anything, but
/// offering a protocol the host cannot speak would break the connection
/// *after* the handshake, which reads as a server fault; refusing it here says
/// what is wrong.
const SUPPORTED_ALPN: [&str; 2] = ["http/1.1", "h2"];

/// The crypto provider every config is built with.
///
/// Pinned to `ring`, the same backend the workspace's `rustls` dependency
/// selects: a second provider would add `aws-lc-rs`'s C/assembly build for no
/// capability gain.
fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Build the client config for one outbound TLS policy.
///
/// * `cert`/`key` — a client certificate chain and its private key. Both or
///   neither: one without the other is an error, not a silently anonymous
///   connection.
/// * `ca` — the roots that verify the peer. `None` keeps the system roots.
/// * `server_name` — the SNI/verification name. `None` uses the URL host.
/// * `alpn` — protocols to offer, filtered against [`SUPPORTED_ALPN`].
/// * `verify: false` — accept any certificate. Only reachable for an app that
///   holds `tls-client` *and* `http-unrestricted`; it is an explicit declared
///   widening, which is why it is implemented at all.
///
/// # Errors
///
/// Unparseable PEM, an incomplete client identity, or an ALPN list the host
/// cannot carry.
pub fn build_client_config(policy: &OutboundTls) -> Result<Arc<rustls::ClientConfig>> {
    let versions: &[&rustls::SupportedProtocolVersion] = match policy.min_version {
        TlsProtocol::Tls13 => &[&rustls::version::TLS13],
        TlsProtocol::Tls12 => &[&rustls::version::TLS13, &rustls::version::TLS12],
    };

    let builder = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(versions)
        .context("configuring TLS protocol versions")?;

    let mut roots = RootCertStore::empty();
    match policy.ca.as_deref() {
        Some(ca) => {
            let certs = parse_certs(ca).context("parsing the client's CA bundle")?;
            for cert in certs {
                roots
                    .add(cert)
                    .context("adding a CA certificate to the client trust store")?;
            }
        }
        None => {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
    }

    let builder = if policy.verify {
        builder.with_root_certificates(roots)
    } else {
        // `dangerous()` is the only way to express `verify: false`, and the
        // bridge requires `http-unrestricted` for it.
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(accept_any::AcceptAnyServerCert::new()))
    };

    let mut config = match (&policy.cert, &policy.key) {
        (Some(cert), Some(key)) => {
            let chain = parse_certs(cert).context("parsing the client certificate chain")?;
            let key = parse_key(key).context("parsing the client private key")?;
            builder
                .with_client_auth_cert(chain, key)
                .context("installing the client certificate and key")?
        }
        (None, None) => builder.with_no_client_auth(),
        (Some(_), None) => anyhow::bail!(
            "tls-client declares a client certificate without its private key: \
             anonymous is not what was asked for"
        ),
        (None, Some(_)) => {
            anyhow::bail!("tls-client declares a client private key without its certificate chain")
        }
    };

    if !policy.alpn.is_empty() {
        let mut protocols = Vec::with_capacity(policy.alpn.len());
        for protocol in &policy.alpn {
            if !SUPPORTED_ALPN.contains(&protocol.as_str()) {
                anyhow::bail!(
                    "tls-client asks for ALPN `{protocol}`, which this host cannot carry \
                     (supported: http/1.1, h2)"
                );
            }
            protocols.push(protocol.as_bytes().to_vec());
        }
        config.alpn_protocols = protocols;
    }

    Ok(Arc::new(config))
}

/// The name to verify a peer against: the policy's `server-name` when it sets
/// one, the URL host otherwise.
///
/// # Errors
///
/// When the resulting name is not a valid DNS name or IP address.
pub fn server_name(policy: Option<&OutboundTls>, host: &str) -> Result<ServerName<'static>> {
    let raw = policy
        .and_then(|policy| policy.server_name.as_deref())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(host)
        .trim()
        .to_string();
    ServerName::try_from(raw.clone())
        .with_context(|| format!("`{raw}` is not a valid TLS server name"))
}

/// Parse a PEM bundle of certificates.
///
/// # Errors
///
/// When the bundle holds no certificate or one does not parse.
pub fn parse_certs(pem: &str) -> Result<Vec<CertificateDer<'static>>> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("reading a PEM certificate")?;
    if certs.is_empty() {
        anyhow::bail!("the PEM bundle holds no certificate");
    }
    Ok(certs)
}

/// Parse the first private key of a PEM bundle.
///
/// # Errors
///
/// When the bundle holds no key or the key is not one rustls accepts.
pub fn parse_key(pem: &str) -> Result<PrivateKeyDer<'static>> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    rustls_pemfile::private_key(&mut reader)
        .context("reading a PEM private key")?
        .ok_or_else(|| anyhow::anyhow!("the PEM bundle holds no private key"))
}

/// The `verify: false` verifier.
///
/// `danger_accept_invalid_certs` in reqwest is not reachable through a
/// preconfigured `rustls::ClientConfig`, so the equivalent lives here: every
/// check returns "valid". It exists because the WIT contract documents
/// `verify: false` and an operator may grant it; refusing to implement it while
/// accepting the grant would be the silent-narrowing shape this framework
/// forbids.
mod accept_any {
    use std::sync::Arc;

    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, Error, SignatureScheme};

    #[derive(Debug)]
    pub(super) struct AcceptAnyServerCert {
        provider: Arc<CryptoProvider>,
    }

    impl AcceptAnyServerCert {
        pub(super) fn new() -> Self {
            Self {
                provider: super::provider(),
            }
        }
    }

    impl ServerCertVerifier for AcceptAnyServerCert {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            // The chain is deliberately not checked, but the handshake
            // signature still is: accepting a certificate without proof of
            // possession would not even be a TLS connection.
            verify_tls12_signature(
                message,
                cert,
                dss,
                &self.provider.signature_verification_algorithms,
            )
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            verify_tls13_signature(
                message,
                cert,
                dss,
                &self.provider.signature_verification_algorithms,
            )
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.provider
                .signature_verification_algorithms
                .supported_schemes()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> OutboundTls {
        OutboundTls {
            verify: true,
            ..OutboundTls::default()
        }
    }

    #[test]
    fn a_system_roots_policy_builds_and_offers_no_alpn() {
        let config = build_client_config(&policy()).expect("system roots build");
        assert!(config.alpn_protocols.is_empty());
    }

    #[test]
    fn a_certificate_without_its_key_is_refused_instead_of_going_anonymous() {
        let err = build_client_config(&OutboundTls {
            cert: Some(fixture("client.crt")),
            ..policy()
        })
        .expect_err("a cert without a key is not an identity");
        assert!(err.to_string().contains("without its private key"), "{err}");
    }

    #[test]
    fn an_unsupported_alpn_is_refused_rather_than_offered() {
        let err = build_client_config(&OutboundTls {
            alpn: vec!["acme/1".to_string()],
            ..policy()
        })
        .expect_err("the host cannot carry acme/1");
        assert!(err.to_string().contains("acme/1"), "{err}");
    }

    #[test]
    fn client_material_ca_and_alpn_are_installed() {
        let config = build_client_config(&OutboundTls {
            cert: Some(fixture("client.crt")),
            key: Some(fixture("client.key")),
            ca: Some(fixture("ca.crt")),
            alpn: vec!["http/1.1".to_string(), "h2".to_string()],
            server_name: Some("localhost".to_string()),
            min_version: TlsProtocol::Tls13,
            verify: true,
        })
        .expect("the fixture material is valid");
        assert_eq!(
            config.alpn_protocols,
            vec![b"http/1.1".to_vec(), b"h2".to_vec()]
        );
        assert!(config.client_auth_cert_resolver.has_certs());
    }

    #[test]
    fn a_garbage_ca_is_an_error_not_a_silent_system_roots_fallback() {
        let err = build_client_config(&OutboundTls {
            ca: Some("not a pem".to_string()),
            ..policy()
        })
        .expect_err("an unparseable CA must fail the call");
        assert!(err.to_string().contains("CA bundle"), "{err}");
    }

    #[test]
    fn server_name_defaults_to_the_url_host_and_honours_the_override() {
        assert_eq!(
            server_name(None, "example.test")
                .expect("host is a name")
                .to_str(),
            "example.test"
        );
        assert_eq!(
            server_name(
                Some(&OutboundTls {
                    server_name: Some("internal.test".to_string()),
                    ..OutboundTls::default()
                }),
                "example.test"
            )
            .expect("override is a name")
            .to_str(),
            "internal.test"
        );
    }

    /// The certificate fixtures live with the server's TLS tests; read them
    /// without copying, and skip loudly when the checkout does not carry them.
    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ferris-server/tests/fixtures/tls")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("reading {} failed: {e}", path.display()))
    }
}
