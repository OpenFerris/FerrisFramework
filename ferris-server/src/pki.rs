//! Internal certificate authority — the host half of device identity
//! (mTLS plan Phase 3 / audit finding H2).
//!
//! Before this module the WIT `pki` imports (`sign-csr`, `ca-cert`, `revoke`,
//! `is-revoked`) existed in the contract but the host answered every call with
//! an "unavailable" error: the guest could *ask* for a certificate and got
//! nothing. Worse, there was no place that owned the CA key, and the TLS
//! `CertificateRevocation` hook had no store behind it, so a revoked device
//! certificate would have kept working until it expired. H2 is exactly that
//! gap.
//!
//! [`HostPki`] fills it:
//!
//! * it owns the operator's CA certificate and key ([`PkiConfig`]) and signs the
//!   PKCS#10 CSRs a plugin submits, so a guest never sees the CA private key;
//! * it keeps a `revoked_certificates` table and an in-memory mirror of it, and
//!   implements [`CertificateRevocation`] so the mirror is consulted during the
//!   TLS handshake (a database round-trip is not possible there);
//! * it implements `PkiProvider` so the WASM host's `pki` methods can delegate
//!   to it without this crate's X.509 dependencies leaking into `ferris-wasm`.
//!   That impl is compiled only with the `wasm` feature; the CA and the
//!   revocation store are useful (and compiled) on every build.
//! * the certificate is **issued to the app, not written by the guest**: the
//!   SAN, the EKUs, the key usages and the CA flag are re-derived from the
//!   calling app and the CSR contributes only its public key. A CSR is a
//!   request, not a template, and `rcgen` copies a CSR's extensions verbatim if
//!   nothing overwrites them.
//! * the validity is capped and one app cannot mint certificates without bound:
//!   [`MAX_ISSUED_TTL_SECONDS`] and [`MAX_ISSUED_PER_APP`] bound what a renewal
//!   loop can accumulate.
//!
//! The provider is process-global (registered once at boot through
//! `ferris_wasm::set_pki_provider`), mirroring how the realtime broadcaster and
//! the notification sender are wired: there is one server process, one CA.
//!
//! Signing uses `rcgen` with the `ring` backend. A consequence worth stating:
//! `ring` cannot sign with an RSA CA key, so the CA must be ECDSA (P-256/P-384)
//! or Ed25519. [`HostPki::load`] validates the CA up front so that misconfiguration
//! fails at boot rather than on the first enrolment.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result, anyhow};
#[cfg(feature = "wasm")]
use async_trait::async_trait;
use ferris_core::{PkiConfig, Value};
use ferris_db::sql::{Column, UpsertBuilder};
use ferris_db::{Db, Stmt};
#[cfg(feature = "wasm")]
use ferris_wasm::PkiProvider;
use rcgen::{CertificateParams, CertificateSigningRequestParams, KeyPair};
use rustls::pki_types::CertificateSigningRequestDer;
use time::{Duration, OffsetDateTime};

use crate::tls::{CertificateRevocation, sha256_hex};

/// Table holding the fingerprints of revoked certificates. The primary key is
/// the fingerprint itself, so revoking the same certificate twice is a no-op
/// (the upsert ignores the conflict) instead of an error.
const REVOKED_TABLE: &str = "revoked_certificates";

/// Table recording which app asked for each issued certificate.
///
/// The CA is install-wide, so without this row any plugin could revoke any
/// other plugin's device certificate. The row is the scope: `revoke` only
/// accepts a fingerprint this app issued.
const ISSUED_TABLE: &str = "issued_certificates";

/// Longest validity this CA issues, seconds.
///
/// The operator's `default_ttl_seconds` is the default; this is the ceiling, so
/// a guest asking for `u32::MAX` cannot mint a credential that outlives every
/// revocation cycle it might be part of. Ninety days matches the renewal window
/// the mTLS plan documents.
const MAX_ISSUED_TTL_SECONDS: i64 = 90 * 24 * 60 * 60;

/// Live certificates one app may hold at the same time.
///
/// A count, not a rate: an app enrolling a fleet of devices needs some headroom
/// and a loop that renews forever must hit a wall. Expired rows stop counting,
/// so the quota frees itself.
const MAX_ISSUED_PER_APP: i64 = 256;

/// The `revoked_certificates` schema, defined once so the DDL and the readers
/// cannot drift.
fn revoked_columns() -> Vec<Column> {
    vec![
        Column::new("fingerprint", "VARCHAR(64)")
            .primary_key()
            .not_null(),
        Column::new("revoked_at", "BIGINT").not_null(),
        Column::new("reason", "TEXT"),
    ]
}

/// The `issued_certificates` schema: fingerprint → issuing app + validity.
fn issued_columns() -> Vec<Column> {
    vec![
        Column::new("fingerprint", "VARCHAR(64)")
            .primary_key()
            .not_null(),
        Column::new("app", "VARCHAR(255)").not_null(),
        Column::new("issued_at", "BIGINT").not_null(),
        Column::new("expires_at", "BIGINT").not_null(),
    ]
}

/// The host's internal CA and the revocation ledger it drives.
pub struct HostPki {
    ca_cert_pem: String,
    ca_key_pem: String,
    default_ttl_seconds: u64,
    pool: Db,
    /// In-memory mirror of `revoked_certificates`, loaded at boot and updated on
    /// every [`HostPki::revoke`]. The TLS verifier needs a **synchronous**
    /// yes/no during the handshake, where awaiting a query is not an option, so
    /// the authoritative table is mirrored here.
    revoked: RwLock<HashSet<String>>,
}

impl std::fmt::Debug for HostPki {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never format `ca_key_pem`: a `{:?}` of this struct must not leak the
        // CA private key into a log line.
        f.debug_struct("HostPki")
            .field("default_ttl_seconds", &self.default_ttl_seconds)
            .field("revoked_count", &self.revoked.read().map_or(0, |r| r.len()))
            .finish_non_exhaustive()
    }
}

impl HostPki {
    /// Load the CA from `cfg` and open the revocation table.
    ///
    /// Fails loudly when a file is unreadable, when the key is one the `ring`
    /// backend cannot sign with, or when the certificate and key do not form a
    /// usable issuer — a server that booted with a broken CA would answer
    /// enrolment with an unhandled error later, after having accepted the
    /// request.
    ///
    /// # Errors
    ///
    /// Returns an error when a CA file cannot be read, when the CA key or
    /// certificate cannot be parsed, or when the revocation table cannot be
    /// created or read. The caller (`Server::run`) treats all of these as fatal
    /// at boot.
    pub async fn load(cfg: &PkiConfig, pool: Db) -> Result<Arc<Self>> {
        let ca_cert_pem = std::fs::read_to_string(&cfg.ca_cert_path).with_context(|| {
            format!("reading the CA certificate {}", cfg.ca_cert_path.display())
        })?;
        let ca_key_pem = std::fs::read_to_string(&cfg.ca_key_path)
            .with_context(|| format!("reading the CA key {}", cfg.ca_key_path.display()))?;

        // Validate the pair by building the issuer once. `self_signed` re-derives
        // the CA `Certificate` from the parsed params; if the key cannot sign
        // with it (e.g. an RSA key under the `ring` backend) this is where it
        // surfaces.
        let ca_key = KeyPair::from_pem(&ca_key_pem).map_err(|e| {
            anyhow!(
                "the CA key {} is not a usable PKCS#8 key: {e}",
                cfg.ca_key_path.display()
            )
        })?;
        issuer(&ca_cert_pem, &ca_key)?;

        pool.ensure_table(REVOKED_TABLE, &revoked_columns())
            .await
            .map_err(|e| anyhow!("creating {REVOKED_TABLE} failed: {e}"))?;
        pool.ensure_table(ISSUED_TABLE, &issued_columns())
            .await
            .map_err(|e| anyhow!("creating {ISSUED_TABLE} failed: {e}"))?;

        let revoked = load_revoked(&pool).await?;
        Ok(Arc::new(Self {
            ca_cert_pem,
            ca_key_pem,
            default_ttl_seconds: cfg.default_ttl_seconds,
            pool,
            revoked: RwLock::new(revoked),
        }))
    }

    /// Sign a base64 DER PKCS#10 CSR for `app` and return the issued
    /// certificate as PEM.
    ///
    /// `ttl_seconds` of `0` selects the CA's configured default, and every
    /// value is clamped to [`MAX_ISSUED_TTL_SECONDS`]. The signature is the
    /// plugin's proof of possession of the CSR's private key: `rcgen` copies the
    /// CSR's public key into the certificate, so only the holder of the matching
    /// private key can use what comes back. Everything else the CSR asked for
    /// (subject attributes, SANs, EKUs, key usages, the CA flag) is replaced by
    /// [`scope_to_app`].
    ///
    /// An app may hold at most [`MAX_ISSUED_PER_APP`] live certificates; past
    /// that the call fails instead of letting a renewal loop grow the ledger.
    ///
    /// The fingerprint is recorded against `app` in `issued_certificates`, which
    /// is what later lets [`Self::revoke`] reject a certificate another app
    /// requested.
    ///
    /// # Errors
    ///
    /// Returns an error when `app` is empty, when the input is not base64, when
    /// the bytes are not a parseable PKCS#10 request, when the CA material
    /// cannot produce a signature, or when the issuance cannot be recorded.
    pub async fn sign(&self, app: &str, csr_der_b64: &str, ttl_seconds: u32) -> Result<String> {
        let app = app.trim();
        if app.is_empty() {
            return Err(anyhow!(
                "a certificate must be issued to a calling app: the app identity is empty"
            ));
        }
        use base64::Engine as _;
        let der = base64::engine::general_purpose::STANDARD
            .decode(csr_der_b64.trim())
            .map_err(|e| anyhow!("the CSR is not valid base64: {e}"))?;
        let csr_der = CertificateSigningRequestDer::from(der);
        let mut csr = CertificateSigningRequestParams::from_der(&csr_der)
            .map_err(|e| anyhow!("the CSR could not be parsed: {e}"))?;
        // The CSR supplies the public key and nothing else that survives into
        // the certificate: see [`scope_to_app`].
        scope_to_app(&mut csr.params, app)?;

        let ttl = if ttl_seconds == 0 {
            self.default_ttl_seconds
        } else {
            u64::from(ttl_seconds)
        };
        let now = OffsetDateTime::now_utc();
        let not_after = now + Duration::seconds(clamp_ttl(ttl));
        csr.params.not_before = now;
        csr.params.not_after = not_after;

        let live = self
            .pool
            .fetch_one(&Stmt::new(
                format!(
                    "SELECT COUNT(*) AS n FROM {ISSUED_TABLE} WHERE app = ? AND expires_at > ?"
                ),
                vec![Value::from(app), Value::from(now.unix_timestamp())],
            ))
            .await
            .map_err(|e| anyhow!("counting the live certificates of `{app}` failed: {e}"))?
            .i64("n")?;
        if live >= MAX_ISSUED_PER_APP {
            return Err(anyhow!(
                "app `{app}` already holds {live} live certificates (limit {MAX_ISSUED_PER_APP}); \
                 reuse one or wait for it to expire"
            ));
        }

        let key = KeyPair::from_pem(&self.ca_key_pem)
            .map_err(|e| anyhow!("the CA key is not a usable PKCS#8 key: {e}"))?;
        let issuer = issuer(&self.ca_cert_pem, &key)?;
        let cert = csr
            .signed_by(&issuer, &key)
            .map_err(|e| anyhow!("the CSR could not be signed: {e}"))?;

        let fingerprint = sha256_hex(cert.der().as_ref());
        let upsert = UpsertBuilder::new(self.pool.dialect(), ISSUED_TABLE)
            .values(vec![
                ("fingerprint", Value::from(fingerprint)),
                ("app", Value::from(app)),
                ("issued_at", Value::from(now.unix_timestamp())),
                ("expires_at", Value::from(not_after.unix_timestamp())),
            ])
            .conflict_on(["fingerprint"])
            .on_conflict_do_nothing()
            .build()
            .map_err(|e| anyhow!("building the issuance record failed: {e}"))?;
        self.pool
            .upsert(upsert)
            .await
            .map_err(|e| anyhow!("persisting the issuance record failed: {e}"))?;

        Ok(cert.pem())
    }

    /// The CA certificate in PEM.
    pub fn ca_cert(&self) -> String {
        self.ca_cert_pem.clone()
    }

    /// Record `fingerprint` as revoked, idempotently, and update the mirror.
    ///
    /// Only the app recorded as the certificate's issuer may revoke it. A
    /// fingerprint that another app issued, or that no app issued, gets the
    /// same refusal — otherwise this endpoint would answer "does this
    /// certificate exist?" for anyone.
    ///
    /// # Errors
    ///
    /// Returns an error when `fingerprint` is not a 64-character hex digest,
    /// when the certificate was not issued to `app`, or when the revocation
    /// cannot be persisted.
    pub async fn revoke(&self, app: &str, fingerprint: &str) -> Result<()> {
        let fp = normalize_fingerprint(fingerprint)?;
        let issued = self
            .pool
            .fetch_optional(&Stmt::new(
                format!("SELECT app FROM {ISSUED_TABLE} WHERE fingerprint = ?"),
                vec![Value::from(fp.as_str())],
            ))
            .await
            .map_err(|e| anyhow!("looking up the certificate issuer failed: {e}"))?;
        let Some(issued) = issued else {
            return Err(anyhow!(
                "certificate {fp} was not issued to this app; only its issuing app can revoke it"
            ));
        };
        let issuer = issued.str("app")?;
        if issuer != app.trim() {
            return Err(anyhow!(
                "certificate {fp} was not issued to this app; only its issuing app can revoke it"
            ));
        }
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let upsert = UpsertBuilder::new(self.pool.dialect(), REVOKED_TABLE)
            .values(vec![
                ("fingerprint", Value::from(fp.as_str())),
                ("revoked_at", Value::from(now)),
                ("reason", Value::Null),
            ])
            .conflict_on(["fingerprint"])
            .on_conflict_do_nothing()
            .build()
            .map_err(|e| anyhow!("building the revocation upsert failed: {e}"))?;
        self.pool
            .upsert(upsert)
            .await
            .map_err(|e| anyhow!("persisting the revocation failed: {e}"))?;
        self.revoked
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(fp);
        Ok(())
    }

    /// Whether the certificate carrying `fingerprint` is revoked.
    fn is_revoked_sync(&self, fingerprint: &str) -> bool {
        let fp = fingerprint.trim().to_ascii_lowercase();
        self.revoked
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&fp)
    }
}

/// The WASM host's `pki` imports delegate here. Compiled only with the `wasm`
/// feature: without a guest runtime nobody can call them, while the TLS side
/// ([`CertificateRevocation`] below) stays available to every build.
#[cfg(feature = "wasm")]
#[async_trait]
impl PkiProvider for HostPki {
    async fn pki_sign_csr(&self, app: &str, csr_der_b64: &str, ttl_seconds: u32) -> Result<String> {
        // One ECDSA signature is microseconds; a blocking-pool hop would cost
        // more than the work it moves.
        self.sign(app, csr_der_b64, ttl_seconds).await
    }

    async fn pki_ca_cert(&self) -> Result<String> {
        Ok(self.ca_cert())
    }

    async fn pki_revoke(&self, app: &str, fingerprint: &str) -> Result<()> {
        self.revoke(app, fingerprint).await
    }

    async fn pki_is_revoked(&self, fingerprint: &str) -> Result<bool> {
        Ok(self.is_revoked_sync(fingerprint))
    }
}

impl CertificateRevocation for HostPki {
    fn is_revoked(&self, fingerprint_hex: &str) -> bool {
        self.is_revoked_sync(fingerprint_hex)
    }
}

/// Build the CA `Certificate` from its PEM cert and an already-parsed key.
fn issuer(ca_cert_pem: &str, ca_key: &KeyPair) -> Result<rcgen::Certificate> {
    let params = CertificateParams::from_ca_cert_pem(ca_cert_pem)
        .map_err(|e| anyhow!("the CA certificate is not usable for signing: {e}"))?;
    params
        .self_signed(ca_key)
        .map_err(|e| anyhow!("the CA certificate and key do not form a usable issuer: {e}"))
}

/// The URI SAN every certificate this CA issues carries: the calling app.
///
/// A URI (not a DNS name) because an app identity is not a host; `spiffe://`
/// makes that explicit and cannot collide with a name a resolver would follow.
fn app_san(app: &str) -> Result<rcgen::SanType> {
    let uri = format!("spiffe://ferris/{app}");
    let value = rcgen::Ia5String::try_from(uri).map_err(|_| {
        anyhow!("the app identity `{app}` is not a valid certificate SAN; check the app name")
    })?;
    Ok(rcgen::SanType::URI(value))
}

/// Replace what the CSR asked for with what the calling app is allowed to have.
///
/// `rcgen` copies the CSR's subject, SANs, EKUs, key usages and CA flag into the
/// certificate it signs. Taken at face value, a guest could get the host's own
/// CA to issue `CN=other-app`, `SAN=admin.internal`, `EKU=serverAuth`, or even a
/// CA certificate — an identity it could then present to every mTLS endpoint
/// that trusts the same CA. The public key is the only part of a CSR that is a
/// fact; the rest is a request.
///
/// The common name is preserved because it is the device identity the mTLS layer
/// reports (`ClientIdentity::common_name`, `SOLMEC-001` and other serials), and
/// it is scoped by the `issued_certificates` row rather than by the CA.
fn scope_to_app(params: &mut rcgen::CertificateParams, app: &str) -> Result<()> {
    let requested_cn = match params.distinguished_name.get(&rcgen::DnType::CommonName) {
        Some(rcgen::DnValue::Utf8String(value)) => Some(value.clone()),
        Some(rcgen::DnValue::PrintableString(value)) => Some(value.as_str().to_string()),
        Some(rcgen::DnValue::Ia5String(value)) => Some(value.as_str().to_string()),
        _ => None,
    };
    let common_name = requested_cn
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value.len() <= 128)
        .unwrap_or_else(|| app.to_string());
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, common_name);
    params.subject_alt_names = vec![app_san(app)?];
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
    params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
    params.is_ca = rcgen::IsCa::NoCa;
    params.name_constraints = None;
    params.crl_distribution_points.clear();
    params.custom_extensions.clear();
    params.serial_number = None;
    Ok(())
}

/// Load the revoked fingerprints already persisted.
async fn load_revoked(pool: &Db) -> Result<HashSet<String>> {
    let rows = pool
        .fetch_all(&Stmt::raw(format!(
            "SELECT fingerprint FROM {REVOKED_TABLE}"
        )))
        .await
        .context("loading revoked certificates failed")?;
    let mut set = HashSet::with_capacity(rows.len());
    for row in &rows {
        set.insert(row.str("fingerprint")?);
    }
    Ok(set)
}

/// Normalise and validate a fingerprint before it is stored or compared.
///
/// Fingerprints are lowercase hex SHA-256 digests (64 hex characters), the same
/// shape [`crate::tls::sha256_hex`] produces and [`crate::tls::ClientIdentity`]
/// exposes. Rejecting anything else keeps a malformed value from silently
/// never matching a real certificate.
fn normalize_fingerprint(fingerprint: &str) -> Result<String> {
    let fp = fingerprint.trim().to_ascii_lowercase();
    if fp.len() != 64 || !fp.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(anyhow!(
            "a certificate fingerprint must be 64 lowercase hex characters, got {fingerprint:?}"
        ));
    }
    Ok(fp)
}

/// Clamp a TTL to the longest validity this CA issues.
///
/// The clamp guards two things at once: an operator config (or a guest request)
/// that would overflow `time::Duration::seconds`, and a certificate that lives
/// longer than the revocation cycle it belongs to.
fn clamp_ttl(seconds: u64) -> i64 {
    i64::try_from(seconds).map_or(MAX_ISSUED_TTL_SECONDS, |s| s.min(MAX_ISSUED_TTL_SECONDS))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CSR is a request: the SAN, the EKUs, the key usages and the CA flag it
    /// asks for are replaced, so the CA never signs a guest's widening.
    #[test]
    fn a_csr_cannot_widen_what_the_issuer_grants() {
        let mut params = rcgen::CertificateParams::new(vec!["admin.internal".to_string()])
            .expect("a CSR with a SAN");
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "SOLMEC-001");
        params
            .distinguished_name
            .push(rcgen::DnType::OrganizationName, "someone else");
        params
            .extended_key_usages
            .push(rcgen::ExtendedKeyUsagePurpose::ServerAuth);
        params.key_usages.push(rcgen::KeyUsagePurpose::KeyCertSign);
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.subject_alt_names = vec![
            rcgen::SanType::DnsName("admin.internal".try_into().expect("name")),
            rcgen::SanType::IpAddress("10.0.0.1".parse().expect("ip")),
        ];

        scope_to_app(&mut params, "app-a").expect("the app name is a valid SAN");

        assert_eq!(
            params.subject_alt_names,
            vec![app_san("app-a").expect("san")],
            "the only SAN is the app's"
        );
        assert_eq!(
            params.extended_key_usages,
            vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth]
        );
        assert_eq!(
            params.key_usages,
            vec![rcgen::KeyUsagePurpose::DigitalSignature]
        );
        assert!(matches!(params.is_ca, rcgen::IsCa::NoCa));
        assert!(
            params
                .distinguished_name
                .get(&rcgen::DnType::OrganizationName)
                .is_none(),
            "subject attributes the issuer did not ask for are dropped"
        );
        assert_eq!(
            params.distinguished_name.get(&rcgen::DnType::CommonName),
            Some(&rcgen::DnValue::Utf8String("SOLMEC-001".to_string())),
            "the device identity the mTLS layer reports is preserved"
        );
    }

    /// A CSR without a usable CN still yields a certificate scoped to the app.
    #[test]
    fn a_missing_common_name_falls_back_to_the_app() {
        let mut params = rcgen::CertificateParams::default();
        params.distinguished_name = rcgen::DistinguishedName::new();
        scope_to_app(&mut params, "app-b").expect("scope");
        assert_eq!(
            params.distinguished_name.get(&rcgen::DnType::CommonName),
            Some(&rcgen::DnValue::Utf8String("app-b".to_string()))
        );
    }

    /// The validity ceiling is the CA's, whatever the caller asks for.
    #[test]
    fn the_ttl_is_capped_at_the_issuer_ceiling() {
        assert_eq!(clamp_ttl(0), 0);
        assert_eq!(clamp_ttl(3600), 3600);
        assert_eq!(clamp_ttl(u64::MAX), MAX_ISSUED_TTL_SECONDS);
        assert_eq!(
            clamp_ttl(u64::from(u32::MAX)),
            MAX_ISSUED_TTL_SECONDS,
            "u32::MAX seconds is still over the ceiling"
        );
    }
}
