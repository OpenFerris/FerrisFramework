//! `pki` interface of [`HostApi`](crate::host::HostApi): forwards to the
//! process-wide internal CA, never holding a key in this crate.

use super::RealHostApi;
use anyhow::Result;

impl RealHostApi {
    // -- pki ----------------------------------------------------------------
    // The `pki` import is part of the WIT contract (Phase 0). The internal CA
    // itself lives in `ferris-server` (it owns the TLS material and the
    // revocation table) and is registered process-wide at boot; this host only
    // forwards. While no provider is registered every call reports the
    // capability as unavailable rather than fabricating a certificate or a
    // revocation answer.
    pub(super) async fn pki_sign_csr(
        &self,
        plugin: &str,
        csr_der_b64: &str,
        ttl_seconds: u32,
    ) -> Result<String> {
        match crate::host::pki_provider() {
            Some(pki) => pki.pki_sign_csr(plugin, csr_der_b64, ttl_seconds).await,
            None => anyhow::bail!(
                "pki service is not configured on this host: no internal CA \
                 (set FERRIS_CA_CERT/FERRIS_CA_KEY)"
            ),
        }
    }
    pub(super) async fn pki_ca_cert(&self) -> Result<String> {
        match crate::host::pki_provider() {
            Some(pki) => pki.pki_ca_cert().await,
            None => anyhow::bail!(
                "pki service is not configured on this host: no internal CA \
                 (set FERRIS_CA_CERT/FERRIS_CA_KEY)"
            ),
        }
    }
    pub(super) async fn pki_revoke(&self, plugin: &str, fingerprint: &str) -> Result<()> {
        match crate::host::pki_provider() {
            Some(pki) => pki.pki_revoke(plugin, fingerprint).await,
            None => anyhow::bail!(
                "pki service is not configured on this host: no revocation store \
                 (set FERRIS_CA_CERT/FERRIS_CA_KEY)"
            ),
        }
    }
    pub(super) async fn pki_is_revoked(&self, fingerprint: &str) -> Result<bool> {
        match crate::host::pki_provider() {
            Some(pki) => pki.pki_is_revoked(fingerprint).await,
            None => anyhow::bail!(
                "pki service is not configured on this host: no revocation store \
                 (set FERRIS_CA_CERT/FERRIS_CA_KEY)"
            ),
        }
    }
}
