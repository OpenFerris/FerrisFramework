//! Signed, expiring download URLs served by the framework over `/files/{id}`.
//!
//! A blob a plugin (or the server) already uploaded lives in the shared object
//! store under a `file_refs` row. To hand that artifact to an **external**
//! system — a webhook receiver, another service, a browser without a session —
//! the framework mints a temporary URL whose authority is an HMAC signature
//! rather than a session cookie:
//!
//! ```text
//! /files/<id>?expires=<unix-seconds>&signature=<base64url-hmac-sha256>
//! ```
//!
//! The signature covers a versioned claim set — the file id, its stored
//! visibility, the signer's tenant and the deadline — so retargeting the URL,
//! extending it, replaying it under another tenant or minting it while the file
//! was public and using it after the file became private all fail to verify.
//! [`verify_path`] rejects an expired value before any bytes are read. The
//! secret is operator-provided (`FERRIS_FILES_SIGNING_SECRET`); with no secret
//! signing is simply off.
//!
//! A signature alone authorizes a **public** blob. A **private** one also
//! requires the caller's [`AccessContext`](crate::store::AccessContext), and its
//! bytes are read through the ACL-checked path, never the unchecked primitive
//! (SEC-24).

// The shared `ferris_core::Error` exceeds clippy's 256-byte threshold, so every
// `Result` in this workspace emits `result_large_err`; boxing only the signer
// would change its public API for no benefit.
#![allow(clippy::result_large_err)]

mod downloads;
mod signer;
#[cfg(test)]
mod tests;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use ferris_core::{AuthError, Error, Result};
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub use downloads::{DownloadedFile, SignedDownloads};
pub use signer::FileUrlSigner;

/// Route prefix the framework mounts signed downloads under.
pub const FILES_ROUTE_PREFIX: &str = "/files";

/// Environment variable carrying the signing secret. Absent → signing disabled.
pub const SIGNING_SECRET_ENV: &str = "FERRIS_FILES_SIGNING_SECRET";

/// Environment variable overriding the default URL lifetime (seconds).
pub const SIGNING_TTL_ENV: &str = "FERRIS_FILES_URL_TTL_SECS";

/// Default lifetime of a minted URL: one hour.
pub(super) const DEFAULT_TTL_SECS: u64 = 3600;

type HmacSha256 = Hmac<Sha256>;

/// The one normalisation applied to a file id before it is signed, verified or
/// looked up: `/files/` addresses the id without a leading slash.
fn normalize_file_id(file_id: &str) -> &str {
    file_id.trim_start_matches('/')
}

/// Everything an attachment URL's HMAC binds.
///
/// The message is versioned and length-prefixed so a tenant id containing the
/// separators cannot shift a field into another position. `is_private` is part
/// of it because a grant minted for a public file must stop working the moment
/// the file becomes private.
struct SignedClaims<'a> {
    file_id: &'a str,
    tenant: Option<&'a str>,
    is_private: bool,
    expires_at: i64,
}

impl SignedClaims<'_> {
    fn message(&self) -> String {
        let tenant = self.tenant.unwrap_or_default();
        format!(
            "v2|{}|{tenant}|{}|{}|{}|{}",
            tenant.len(),
            self.file_id.len(),
            self.file_id,
            u8::from(self.is_private),
            self.expires_at
        )
    }
}

fn sign_claims(claims: &SignedClaims<'_>, secret: &[u8]) -> Result<String> {
    let mut mac = HmacSha256::new_from_slice(secret).map_err(|e| {
        Error::Auth(AuthError::TokenInvalid(format!(
            "cannot initialize HMAC: {e}"
        )))
    })?;
    mac.update(claims.message().as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

/// Verify `signature` against `claims`, **and** that it has not expired.
/// Constant-time comparison through the digest crate's `verify_slice`.
fn verify_claims(claims: &SignedClaims<'_>, secret: &[u8], signature: &str) -> bool {
    if claims.expires_at < Utc::now().timestamp() {
        return false;
    }
    let Ok(mut mac) = HmacSha256::new_from_slice(secret) else {
        return false;
    };
    mac.update(claims.message().as_bytes());
    let Ok(provided) = URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    mac.verify_slice(&provided).is_ok()
}

/// URL-safe, unpadded HMAC-SHA256 of a **path-only** grant (file id and
/// deadline, no visibility or tenant).
///
/// Kept as the generic primitive; attachment URLs go through [`FileUrlSigner`],
/// whose claims bind the visibility and tenant too (SEC-24).
///
/// # Errors
///
/// Returns an internal error only if the HMAC backend rejects the key, which
/// cannot happen for HMAC (any key length is valid); the `Result` keeps the
/// caller panic-free.
pub fn sign_path(secret: &[u8], path: &str, expires_at: i64) -> Result<String> {
    sign_claims(
        &SignedClaims {
            file_id: path,
            tenant: None,
            is_private: false,
            expires_at,
        },
        secret,
    )
}

/// Verify a signature produced by [`sign_path`], **and** that it has not
/// expired.
#[must_use]
pub fn verify_path(secret: &[u8], path: &str, expires_at: i64, signature: &str) -> bool {
    verify_claims(
        &SignedClaims {
            file_id: path,
            tenant: None,
            is_private: false,
            expires_at,
        },
        secret,
        signature,
    )
}
