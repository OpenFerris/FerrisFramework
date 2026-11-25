//! `[auth]` section.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub password_min_length: usize,
    /// Required character classes for a password. The defaults are the ones the
    /// server effectively enforced before `[auth]` exposed them, so a config
    /// without these keys keeps its old behaviour.
    #[serde(default = "default_true")]
    pub password_require_uppercase: bool,
    #[serde(default = "default_true")]
    pub password_require_lowercase: bool,
    #[serde(default = "default_true")]
    pub password_require_numbers: bool,
    #[serde(default)]
    pub password_require_symbols: bool,
    /// Sliding idle window for a session, in minutes. `30` is the value the
    /// server used before the key existed.
    #[serde(default = "default_session_idle_timeout_minutes")]
    pub session_idle_timeout_minutes: u64,
    /// Absolute session lifetime, in minutes. `60` is the core default.
    #[serde(default = "default_session_timeout_minutes")]
    pub session_timeout_minutes: u64,
    /// Failed attempts before the account lockout, and its duration.
    #[serde(default = "default_max_login_attempts")]
    pub max_login_attempts: u32,
    #[serde(default = "default_lockout_duration_minutes")]
    pub lockout_duration_minutes: u64,
    /// Concurrent sessions one account may hold (`0` disables the cap).
    #[serde(default = "default_max_sessions_per_user")]
    pub max_sessions_per_user: u32,
    /// Issuer shown by authenticator apps in the `otpauth://` URI.
    #[serde(default = "default_mfa_issuer")]
    pub mfa_issuer: String,
    /// Opt-in gate on a verified email address. `false` keeps the historical
    /// behaviour, where verification is self-service only and never blocks a
    /// login.
    #[serde(default)]
    pub require_email_verification: bool,
    /// OAuth providers the Desk may log in with, in the core wire shape so the
    /// same `[auth.oauth]` block works for the CLI and the server (SRV3).
    /// Empty means "no provider mounted"; the router keeps its `Unimplemented`
    /// fallback in that case.
    #[serde(default)]
    pub oauth: ferris_core::OAuthConfig,
}

impl fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthConfig")
            .field("password_min_length", &self.password_min_length)
            .field(
                "password_require_uppercase",
                &self.password_require_uppercase,
            )
            .field(
                "password_require_lowercase",
                &self.password_require_lowercase,
            )
            .field("password_require_numbers", &self.password_require_numbers)
            .field("password_require_symbols", &self.password_require_symbols)
            .field(
                "session_idle_timeout_minutes",
                &self.session_idle_timeout_minutes,
            )
            .field("session_timeout_minutes", &self.session_timeout_minutes)
            .field("max_login_attempts", &self.max_login_attempts)
            .field("lockout_duration_minutes", &self.lockout_duration_minutes)
            .field("max_sessions_per_user", &self.max_sessions_per_user)
            .field("mfa_issuer", &self.mfa_issuer)
            .field(
                "require_email_verification",
                &self.require_email_verification,
            )
            .field("oauth", &self.oauth)
            .finish()
    }
}

const fn default_true() -> bool {
    true
}

const fn default_session_idle_timeout_minutes() -> u64 {
    30
}

const fn default_session_timeout_minutes() -> u64 {
    60
}

const fn default_max_login_attempts() -> u32 {
    5
}

const fn default_lockout_duration_minutes() -> u64 {
    15
}

const fn default_max_sessions_per_user() -> u32 {
    5
}

fn default_mfa_issuer() -> String {
    "Ferris".to_string()
}
