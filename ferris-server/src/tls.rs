//! Server-side TLS termination with optional client-certificate (mTLS)
//! authentication — the host half of device identity.
//!
//! The host stays identity-agnostic: it verifies that a client certificate
//! chains to the operator-configured CA(s), and it exposes the certificate's
//! *facts* (subject, common name, issuer, SHA-256 fingerprint) as
//! [`ClientIdentity`]. It knows nothing about gateways, enrolment tokens or
//! rotation — that is the plugin's domain.
//!
//! Wiring: [`build_server_config`] turns a [`ferris_core::TlsConfig`] into a
//! `rustls` server config; [`TlsListener`] wraps the TCP listener and performs
//! the handshake per connection, carrying the verified identity as the
//! connection's `ConnectInfo` (`TlsClientInfo`) so a downstream extractor can
//! read it off the request.
//!
//! [`build_server_config`] takes an optional [`CertificateRevocation`] so a
//! certificate can be refused during the handshake, not merely rejected by a
//! later permission check. The store behind that trait is an operator concern
//! (see the plan's Phase 3); when `None`, chain validation alone decides.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::extract::connect_info::Connected;
use axum::serve::{IncomingStream, Listener};
use rustls::RootCertStore;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CertificateError, DigitallySignedStruct, DistinguishedName, Error, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tracing::warn;

use ferris_core::{TlsConfig, TlsMinVersion};

pub use ferris_core::ClientIdentity;

/// Extract the identity facts from a DER-encoded X.509 certificate.
///
/// Returns `None` when the bytes are not a parseable certificate. rustls has
/// already validated the chain by the time this is called, so a parse failure
/// means the certificate is malformed in a way rustls did not care about, and
/// the safe answer is "no identity".
pub fn client_identity_from_der(der: &[u8]) -> Option<ClientIdentity> {
    let (_, cert) = x509_parser::parse_x509_certificate(der).ok()?;
    let common_name = cert
        .subject()
        .iter_common_name()
        .next()
        .map(|attr| attr.as_str().ok().map(str::to_string))
        .unwrap_or(None);
    Some(ClientIdentity {
        subject: cert.subject().to_string(),
        common_name,
        fingerprint: sha256_hex(der),
        issuer: cert.issuer().to_string(),
    })
}

/// Lowercase hex SHA-256 digest. Used for the certificate fingerprint that a
/// revocation store keys on.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Per-connection information axum surfaces as
/// `ConnectInfo<TlsClientInfo>`. `peer` is the TCP peer; `identity` is the
/// verified client certificate, if any.
#[derive(Debug, Clone)]
pub struct TlsClientInfo {
    /// The remote TCP address. For [`TlsListener::local_addr`] this is the
    /// bound address, not a peer.
    pub peer: SocketAddr,
    /// The verified client identity, absent for anonymous connections.
    pub identity: Option<ClientIdentity>,
}

/// A store that decides whether a client certificate is revoked, consulted
/// during the handshake so a revoked certificate never reaches a request.
///
/// Keyed by the same lowercase hex SHA-256 fingerprint [`ClientIdentity`]
/// exposes. The implementation is the operator's (a table, a CRL, a KMS);
/// the host only needs a synchronous yes/no at handshake time.
pub trait CertificateRevocation: fmt::Debug + Send + Sync {
    fn is_revoked(&self, fingerprint_hex: &str) -> bool;
}

/// Consults several [`CertificateRevocation`] stores; a certificate is revoked
/// if **any** of them says so.
///
/// The internal CA's ledger ([`crate::pki::HostPki`]) and an operator-supplied
/// store can then coexist in one handshake check instead of one silently
/// overriding the other. An empty set revokes nothing, which is why
/// [`build_server_config`] is still passed `None` rather than an empty
/// combinator when no store is configured.
#[derive(Debug)]
pub struct CombinedRevocation(Vec<Arc<dyn CertificateRevocation>>);

impl CombinedRevocation {
    #[must_use]
    pub fn new(stores: Vec<Arc<dyn CertificateRevocation>>) -> Self {
        Self(stores)
    }
}

impl CertificateRevocation for CombinedRevocation {
    fn is_revoked(&self, fingerprint_hex: &str) -> bool {
        self.0.iter().any(|store| store.is_revoked(fingerprint_hex))
    }
}

/// Wraps a [`ClientCertVerifier`], adding a post-chain-check revocation test.
///
/// The chain is verified first (and its error returned unchanged); only a
/// certificate that is otherwise valid can be refused for being revoked.
#[derive(Debug)]
struct RevocationVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    revocation: Arc<dyn CertificateRevocation>,
}

impl ClientCertVerifier for RevocationVerifier {
    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }

    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, Error> {
        let verified = self
            .inner
            .verify_client_cert(end_entity, intermediates, now)?;
        if self.revocation.is_revoked(&sha256_hex(end_entity.as_ref())) {
            return Err(Error::InvalidCertificate(CertificateError::Revoked));
        }
        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// Build a `rustls` server config from the operator's [`TlsConfig`].
///
/// * `cert_path` / `key_path` are the server's own certificate and key.
/// * `client_ca_path` enables client authentication; `require_client_cert`
///   makes it mandatory (mTLS) instead of optional.
/// * `revocation`, when set, additionally refuses revoked client certificates
///   during the handshake.
///
/// Fails loudly on a missing/unreadable file, an empty PEM bundle, an invalid
/// `min_version`, or `require_client_cert` without a client CA — a server that
/// silently degraded to "no client auth" would be worse than one that refuses
/// to start.
pub fn build_server_config(
    cfg: &TlsConfig,
    revocation: Option<Arc<dyn CertificateRevocation>>,
) -> Result<Arc<rustls::ServerConfig>> {
    let certs = load_certs(&cfg.cert_path)
        .with_context(|| format!("loading server certificate chain {:?}", cfg.cert_path))?;
    let key = load_key(&cfg.key_path)
        .with_context(|| format!("loading server private key {:?}", cfg.key_path))?;

    let provider = Arc::new(rustls::crypto::ring::default_provider());

    let versions: &[&rustls::SupportedProtocolVersion] =
        match cfg.min_version().map_err(anyhow::Error::msg)? {
            TlsMinVersion::Tls13 => &[&rustls::version::TLS13],
            TlsMinVersion::Tls12 => &[&rustls::version::TLS13, &rustls::version::TLS12],
        };

    let verifier_builder = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(versions)
        .context("configuring TLS protocol versions")?;

    // `WantsServerCert` is the stage after a client-cert decision has been made.
    let server_cert_builder = match cfg.client_ca_path.as_deref() {
        Some(ca_path) => {
            let roots = Arc::new(
                load_root_store(ca_path)
                    .with_context(|| format!("loading client CA bundle {:?}", ca_path))?,
            );
            let client_verifier_builder =
                WebPkiClientVerifier::builder_with_provider(roots, provider);
            let client_verifier_builder = if cfg.require_client_cert {
                // mTLS: anonymous clients are refused.
                client_verifier_builder
            } else {
                // Offer client authentication but accept anonymous clients too.
                client_verifier_builder.allow_unauthenticated()
            };
            let verifier = client_verifier_builder
                .build()
                .context("building client certificate verifier")?;
            // Chain validation first; revocation is an additional gate on an
            // otherwise-valid certificate.
            let verifier: Arc<dyn ClientCertVerifier> = match revocation {
                Some(revocation) => Arc::new(RevocationVerifier {
                    inner: verifier,
                    revocation,
                }),
                None => verifier,
            };
            verifier_builder.with_client_cert_verifier(verifier)
        }
        None => {
            if cfg.require_client_cert {
                bail!(
                    "tls.require_client_cert is set but tls.client_ca_path is missing: \
                     there is no CA to verify client certificates against"
                );
            }
            verifier_builder.with_no_client_auth()
        }
    };

    let config = server_cert_builder
        .with_single_cert(certs, key)
        .context("installing the server certificate and key")?;
    Ok(Arc::new(config))
}

/// Maximum time the TLS handshake may take once the TCP connection exists.
///
/// `rustls` drives the handshake to completion inside `accept`, so a client
/// that opens the socket and never sends a `ClientHello` would otherwise hold
/// the listener's accept loop indefinitely: an unauthenticated remote can stall
/// every new connection with one idle TCP socket. Ten seconds is far above a
/// real handshake (milliseconds on a LAN) and far below a useful stall.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// A TCP listener that performs the TLS handshake on accept and reports the
/// verified client identity as its [`Listener::Addr`].
pub struct TlsListener {
    inner: TcpListener,
    acceptor: TlsAcceptor,
    handshake_timeout: Duration,
}

impl fmt::Debug for TlsListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsListener").finish_non_exhaustive()
    }
}

impl TlsListener {
    /// Wrap a bound TCP listener with the given TLS server config.
    pub fn new(inner: TcpListener, config: Arc<rustls::ServerConfig>) -> Self {
        Self {
            inner,
            acceptor: TlsAcceptor::from(config),
            handshake_timeout: HANDSHAKE_TIMEOUT,
        }
    }

    /// Override the handshake deadline (tests use a short one).
    #[must_use]
    pub fn with_handshake_timeout(mut self, timeout: Duration) -> Self {
        self.handshake_timeout = timeout;
        self
    }

    /// Accept exactly one connection and complete its handshake.
    ///
    /// Unlike [`Listener::accept`], a failed handshake (missing, untrusted or
    /// revoked client certificate) is returned as an `Err` instead of being
    /// swallowed, which is what makes the rejection observable to tests and to
    /// callers that want their own retry/log policy. A handshake that exceeds
    /// [`HANDSHAKE_TIMEOUT`] is dropped and reported as
    /// [`io::ErrorKind::TimedOut`]; [`Listener::accept`] logs it and keeps
    /// serving.
    pub async fn accept_tls(&mut self) -> io::Result<(TlsStream<TcpStream>, TlsClientInfo)> {
        let (tcp, peer) = self.inner.accept().await?;
        // rustls drives the full handshake (including client-cert verification)
        // before this returns, so by the time we build the identity the peer has
        // proven possession of the key in its certificate. The timeout is what
        // keeps a silent peer from pinning the accept loop (SRV-009).
        let stream =
            match tokio::time::timeout(self.handshake_timeout, self.acceptor.accept(tcp)).await {
                Ok(Ok(stream)) => stream,
                Ok(Err(error)) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
                Err(_elapsed) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "TLS handshake exceeded the handshake deadline",
                    ));
                }
            };
        let identity = peer_identity(&stream);
        Ok((stream, TlsClientInfo { peer, identity }))
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = TlsClientInfo;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.accept_tls().await {
                Ok(connection) => return connection,
                Err(e) => {
                    // A failed handshake is per-connection: log it and keep
                    // serving. Only a transport-level accept failure is worth a
                    // backoff, so we do not spin on a resource error.
                    warn!(error = %e, "rejected TLS connection");
                    if e.kind() != io::ErrorKind::InvalidData {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr().map(|addr| TlsClientInfo {
            peer: addr,
            identity: None,
        })
    }
}

/// Bridges the connection's identity into axum's `ConnectInfo` mechanism: a
/// route (or middleware) can extract `ConnectInfo<TlsClientInfo>` to read it.
impl Connected<IncomingStream<'_, TlsListener>> for TlsClientInfo {
    fn connect_info(stream: IncomingStream<'_, TlsListener>) -> Self {
        stream.remote_addr().clone()
    }
}

fn peer_identity(stream: &TlsStream<TcpStream>) -> Option<ClientIdentity> {
    let (_, connection) = stream.get_ref();
    let certificate = connection.peer_certificates()?.first()?;
    client_identity_from_der(certificate.as_ref())
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let data = std::fs::read(path)?;
    let mut reader = io::BufReader::new(data.as_slice());
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("reading PEM certificates")?;
    if certs.is_empty() {
        bail!("no certificates found in {}", path.display());
    }
    Ok(certs)
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let data = std::fs::read(path)?;
    let mut reader = io::BufReader::new(data.as_slice());
    rustls_pemfile::private_key(&mut reader)
        .context("reading PEM private key")?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {}", path.display()))
}

fn load_root_store(path: &Path) -> Result<RootCertStore> {
    let data = std::fs::read(path)?;
    let mut reader = io::BufReader::new(data.as_slice());
    let mut roots = RootCertStore::empty();
    let mut added = 0usize;
    for cert in rustls_pemfile::certs(&mut reader) {
        let cert = cert.context("reading a CA certificate")?;
        roots.add(cert).context("adding a CA certificate")?;
        added += 1;
    }
    if added == 0 {
        bail!("no CA certificates found in {}", path.display());
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/tls")
            .join(name)
    }

    fn first_cert_der(name: &str) -> Vec<u8> {
        let pem = std::fs::read(fixture(name)).unwrap();
        let mut reader = io::BufReader::new(pem.as_slice());
        rustls_pemfile::certs(&mut reader)
            .next()
            .expect("a certificate")
            .expect("a valid PEM certificate")
            .to_vec()
    }

    fn server_cfg(require_client_cert: bool, client_ca: Option<&str>) -> TlsConfig {
        TlsConfig {
            cert_path: fixture("server.crt"),
            key_path: fixture("server.key"),
            client_ca_path: client_ca.map(fixture),
            require_client_cert,
            min_version: None,
        }
    }

    #[test]
    fn min_version_defaults_to_tls12_and_rejects_nonsense() {
        let mut cfg = TlsConfig::default();
        assert_eq!(cfg.min_version().unwrap(), TlsMinVersion::Tls12);

        cfg.min_version = Some("1.3".into());
        assert_eq!(cfg.min_version().unwrap(), TlsMinVersion::Tls13);

        cfg.min_version = Some("2.0".into());
        assert!(cfg.min_version().is_err());
    }

    #[test]
    fn identity_is_extracted_from_a_der_certificate() {
        let der = first_cert_der("client.crt");
        let identity = client_identity_from_der(&der).expect("parseable certificate");

        assert_eq!(identity.common_name.as_deref(), Some("SOLMEC-001"));
        assert!(
            identity.subject.contains("SOLMEC-001"),
            "subject was {:?}",
            identity.subject
        );
        assert!(
            identity.issuer.contains("Ferris Test Root CA"),
            "issuer was {:?}",
            identity.issuer
        );
        // SHA-256 hex.
        assert_eq!(identity.fingerprint.len(), 64);
        assert!(identity.fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn an_unparseable_certificate_yields_no_identity() {
        assert!(client_identity_from_der(b"not a certificate").is_none());
    }

    #[test]
    fn requiring_a_client_cert_without_a_ca_is_a_boot_error() {
        let cfg = server_cfg(true, None);
        let err = build_server_config(&cfg, None).expect_err("must refuse to start");
        assert!(
            err.to_string().contains("client_ca_path"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn server_only_tls_needs_no_client_ca() {
        let cfg = server_cfg(false, None);
        assert!(build_server_config(&cfg, None).is_ok());
    }

    #[test]
    fn a_client_ca_builds_an_mtls_config() {
        let cfg = server_cfg(true, Some("ca.crt"));
        assert!(build_server_config(&cfg, None).is_ok());
    }

    #[test]
    fn a_missing_certificate_file_is_an_error() {
        let mut cfg = server_cfg(false, None);
        cfg.cert_path = fixture("does-not-exist.crt");
        assert!(build_server_config(&cfg, None).is_err());
    }

    /// SRV-009: a peer that opens the TCP socket and never sends a
    /// `ClientHello` must be dropped at the handshake deadline instead of
    /// blocking the accept loop forever. The deadline is shortened so the
    /// test does not wait the production ten seconds.
    #[tokio::test]
    async fn a_silent_client_is_dropped_at_the_handshake_deadline() {
        let config = build_server_config(&server_cfg(false, None), None)
            .expect("the fixture builds a config");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("the bound address");
        let mut tls =
            TlsListener::new(listener, config).with_handshake_timeout(Duration::from_millis(100));
        // Hold the client socket open without writing a single byte.
        let _silent = TcpStream::connect(addr).await.expect("connect");

        match tls.accept_tls().await {
            Ok(_) => panic!("a peer that sends no ClientHello cannot complete a handshake"),
            Err(error) => assert_eq!(error.kind(), io::ErrorKind::TimedOut),
        }
    }
}
