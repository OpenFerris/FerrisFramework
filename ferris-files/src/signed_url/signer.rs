//! Mint and verify temporary URLs for blobs in the shared file store.

use std::time::Duration;

use chrono::Utc;
use ferris_core::Result;

use super::{
    DEFAULT_TTL_SECS, FILES_ROUTE_PREFIX, SIGNING_SECRET_ENV, SIGNING_TTL_ENV, SignedClaims,
    normalize_file_id, sign_claims, verify_claims,
};

/// Mints and verifies temporary URLs for blobs in the shared file store.
///
/// `Clone` is cheap (the secret is a small `Vec`), so a signer can live inside
/// the per-request host API without ceremony.
#[derive(Clone)]
pub struct FileUrlSigner {
    pub(super) secret: Vec<u8>,
    pub(super) default_ttl: Duration,
    prefix: String,
    /// Tenant this signer belongs to. A URL minted under one tenant never
    /// verifies under another even when both share the same secret.
    tenant: Option<String>,
}

impl std::fmt::Debug for FileUrlSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the secret.
        f.debug_struct("FileUrlSigner")
            .field("secret", &"<redacted>")
            .field("default_ttl", &self.default_ttl)
            .field("prefix", &self.prefix)
            .field("tenant", &self.tenant)
            .finish()
    }
}

impl FileUrlSigner {
    /// Build a signer from a secret and a default URL lifetime.
    #[must_use]
    pub fn new(secret: impl Into<Vec<u8>>, default_ttl: Duration) -> Self {
        Self {
            secret: secret.into(),
            default_ttl,
            prefix: FILES_ROUTE_PREFIX.to_string(),
            tenant: None,
        }
    }

    /// Scope every URL this signer mints and verifies to one tenant.
    #[must_use]
    pub fn with_tenant(mut self, tenant: impl Into<String>) -> Self {
        let tenant = tenant.into();
        self.tenant = (!tenant.trim().is_empty()).then_some(tenant);
        self
    }

    /// The tenant scope, when the deployment is tenant-aware.
    #[must_use]
    pub fn tenant(&self) -> Option<&str> {
        self.tenant.as_deref()
    }

    /// Build a signer from the environment, or `None` when
    /// [`SIGNING_SECRET_ENV`] is unset/blank (signing disabled).
    ///
    /// This is the single resolution point shared by the web tier that serves
    /// `/files/…` and the plugin host that mints the URLs, so both always agree
    /// on the secret.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let secret = std::env::var(SIGNING_SECRET_ENV).ok()?;
        if secret.trim().is_empty() {
            return None;
        }
        let ttl_secs = std::env::var(SIGNING_TTL_ENV)
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(DEFAULT_TTL_SECS);
        Some(Self::new(
            secret.into_bytes(),
            Duration::from_secs(ttl_secs),
        ))
    }

    /// Whether the signer holds a non-empty secret. An empty secret would make
    /// every signature trivially forgeable, so it is treated as "off".
    #[must_use]
    pub const fn is_usable(&self) -> bool {
        !self.secret.is_empty()
    }

    /// A temporary URL for `file_id` using the default lifetime.
    ///
    /// # Errors
    ///
    /// Propagates the (unreachable in practice) HMAC failure from [`sign_path`].
    pub fn signed_url(&self, file_id: &str) -> Result<String> {
        self.signed_url_with_ttl(file_id, self.default_ttl)
    }

    /// A temporary URL for a **public** `file_id` valid for `ttl`.
    ///
    /// # Errors
    ///
    /// Propagates the (unreachable in practice) HMAC failure from [`sign_path`].
    pub fn signed_url_with_ttl(&self, file_id: &str, ttl: Duration) -> Result<String> {
        self.signed_url_for_with_ttl(file_id, false, ttl)
    }

    /// A URL for `file_id` bound to its stored visibility.
    ///
    /// # Errors
    ///
    /// Propagates the (unreachable in practice) HMAC failure from [`sign_path`].
    pub fn signed_url_for(&self, file_id: &str, is_private: bool) -> Result<String> {
        self.signed_url_for_with_ttl(file_id, is_private, self.default_ttl)
    }

    /// A URL for `file_id` bound to `is_private` and valid for `ttl`.
    ///
    /// The id is normalised (a leading `/` is dropped) **before** it is signed
    /// and before it is placed in the path, so the id the verifier receives
    /// from `/files/<id>` is byte-for-byte the one the signature covers.
    ///
    /// # Errors
    ///
    /// Propagates the (unreachable in practice) HMAC failure from [`sign_path`].
    pub fn signed_url_for_with_ttl(
        &self,
        file_id: &str,
        is_private: bool,
        ttl: Duration,
    ) -> Result<String> {
        let expires = Utc::now()
            .timestamp()
            .saturating_add(i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX));
        let file_id = normalize_file_id(file_id);
        let signature = sign_claims(
            &SignedClaims {
                file_id,
                tenant: self.tenant.as_deref(),
                is_private,
                expires_at: expires,
            },
            &self.secret,
        )?;
        Ok(format!(
            "{}/{file_id}?expires={expires}&signature={signature}",
            self.prefix.trim_end_matches('/')
        ))
    }

    /// Whether `(file_id, expires, signature)` is a live, untampered grant for
    /// a **public** file.
    #[must_use]
    pub fn verify(&self, file_id: &str, expires: i64, signature: &str) -> bool {
        self.verify_for(file_id, false, expires, signature)
    }

    /// Whether the grant covers `file_id` at its stored `is_private` value,
    /// for this signer's tenant and before the deadline.
    #[must_use]
    pub fn verify_for(
        &self,
        file_id: &str,
        is_private: bool,
        expires: i64,
        signature: &str,
    ) -> bool {
        self.is_usable()
            && verify_claims(
                &SignedClaims {
                    file_id: normalize_file_id(file_id),
                    tenant: self.tenant.as_deref(),
                    is_private,
                    expires_at: expires,
                },
                &self.secret,
                signature,
            )
    }
}
