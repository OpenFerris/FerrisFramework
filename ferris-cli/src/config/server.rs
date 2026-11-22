//! `[server]` section.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub workers: usize,
    pub request_timeout_secs: u64,
    /// Maximum request body in mebibytes. `serve` copies this into both
    /// `CoreConfig.server` and `ServerSettings`, which is the field the router
    /// actually enforces.
    #[serde(default = "default_body_limit_mb")]
    pub body_limit_mb: usize,
    pub roles: Vec<String>,
    /// Enable CORS. Off by default (same-origin); when on, only the explicit
    /// `cors_origins` allow-list is honored — there is no wildcard mode.
    #[serde(default)]
    pub enable_cors: bool,
    /// Explicit CORS origin allow-list. `*` entries are refused at the router.
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// Server-side TLS. `None` serves plain HTTP (the dev default). `Some`
    /// terminates TLS in-process, and — with `client_ca_path` — authenticates
    /// clients by certificate (mTLS). Populated from `[server.tls]` in
    /// `ferris.toml` or the `FERRIS_TLS_*` / `FERRIS_MTLS_*` environment.
    #[serde(default)]
    pub tls: Option<ferris_core::TlsConfig>,
    /// Internal certificate authority that signs device CSRs. Populated from
    /// `[server.pki]` in `ferris.toml` or the `FERRIS_CA_*` environment. `None`
    /// leaves the plugin `pki` imports unavailable.
    #[serde(default)]
    pub pki: Option<ferris_core::PkiConfig>,
    /// Public origin the deployment is served at (`https://erp.example.com`),
    /// used for absolute URLs that leave the process — OAuth redirect URIs.
    /// `None` derives it from the bind `host:port` and the TLS setting, which
    /// is only correct locally; behind a proxy or a wildcard bind it must be
    /// set explicitly (SRV2).
    #[serde(default)]
    pub external_url: Option<String>,
}

pub(super) const fn default_body_limit_mb() -> usize {
    50
}
