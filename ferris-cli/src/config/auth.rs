//! `[auth]` section.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// JWT signing secret. There is no usable default: it must be provided via
    /// the environment (`FERRIS_JWT_SECRET` / `JWT_SECRET`) or the config file.
    /// A placeholder value is rejected outright at load time.
    pub jwt_secret: Option<String>,
    pub jwt_expiry_hours: u64,
    pub refresh_expiry_days: u64,
    pub password_min_length: usize,
    pub mfa_enabled: bool,
    /// OAuth providers the Desk may log in with, in the core wire shape so the
    /// same `[auth.oauth]` block works for the CLI and the server (SRV3).
    /// Empty means "no provider mounted"; the router keeps its `Unimplemented`
    /// fallback in that case.
    #[serde(default)]
    pub oauth: ferris_core::OAuthConfig,
}

impl fmt::Debug for AuthConfig {
    /// N3: `jwt_secret` signed every session token; showing it through
    /// `{:?}` was one log line away from a full takeover.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthConfig")
            .field("jwt_secret", &"<redacted>")
            .field("jwt_expiry_hours", &self.jwt_expiry_hours)
            .field("refresh_expiry_days", &self.refresh_expiry_days)
            .field("password_min_length", &self.password_min_length)
            .field("mfa_enabled", &self.mfa_enabled)
            .field("oauth", &self.oauth)
            .finish()
    }
}
