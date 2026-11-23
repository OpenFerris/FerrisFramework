//! Settings for a running `ferris` server instance.

use std::sync::Arc;

use anyhow::Result;
use ferris_core::{Config, DatabaseConfig, I18n, RealtimeConfig};
use ferris_db::Db;
#[cfg(feature = "wasm")]
use ferris_wasm::PluginStore;
use tracing::warn;

use crate::roles::ServerRoles;
use crate::tls::CertificateRevocation;

/// Settings for a running `ferris` server instance.
#[derive(Debug, Clone)]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
    pub roles: ServerRoles,
    pub enable_cors: bool,
    pub cors_origins: Vec<String>,
    pub request_timeout_secs: u64,
    pub body_limit_mb: usize,
    pub database: DatabaseConfig,
    pub realtime: RealtimeConfig,
    /// Full app config (for auth, etc.)
    pub config: Config,
    /// Optional plugin store for plugin routes.
    ///
    /// `ferris dev` publishes the artifact it just built and hands the store
    /// over; `ferris serve`/`run` build it from `[apps]` before
    /// [`Server::run`](Server::run) (`ferris_cli::commands::plugin_boot`), so
    /// `build_router` mounts `/plugins` and registers `/api/method/<name>` from
    /// the first request instead of never.
    ///
    /// Absent when the crate was built without the `wasm` feature: there is no
    /// guest runtime to hand a store to, so the field does not exist either
    /// (a field that could only ever be `None` would invite a caller to set it
    /// and silently get nothing).
    #[cfg(feature = "wasm")]
    pub plugins: Option<Arc<PluginStore>>,
    /// Shared database pool to reuse across the API and dev plugins. When unset
    /// the server opens its own pool from `database`.
    pub pool: Option<Db>,
    /// Dev mode: serve `/static/desk.css` live from this generated file instead
    /// of the stylesheet compiled into the binary. Lets theme/CSS edits apply
    /// on refresh without a server restart.
    pub desk_css_dev: Option<std::path::PathBuf>,
    /// Dev mode: enables affordances that are only correct while the desk is
    /// rebuilt in place. Set by `ferris dev`, never by `ferris serve`.
    ///
    /// Today it drives `Cache-Control: no-store` on `/pkg`. The hydrate bundle
    /// keeps the same file names across rebuilds, so without it a browser can
    /// reuse the previous `ferris-web.js` against the freshly built wasm and
    /// then fail to instantiate with `LinkError: import object field ... is not
    /// a Function` as soon as a new `web-sys` import enters the hydrate graph:
    /// the glue has to be generated from the very wasm it loads.
    pub dev: bool,
    /// Optional policy document loaded on top of the DocType-declared rules.
    ///
    /// This is the operator's lever: grant or deny a right without recompiling a
    /// DocType. Missing file is not an error — most installs will not have one.
    /// A file that exists but does not parse is logged at warn level by
    /// `install_policies_from_path`, and the server still starts with the
    /// DocType-declared rules alone, because a policy the engine cannot read
    /// grants nothing and booting is not made safer by refusing.
    pub policy_path: Option<std::path::PathBuf>,
    /// Client-certificate revocation store, consulted during the TLS handshake
    /// when the server terminates mTLS. `None` means chain validation alone
    /// decides (no revocation list configured).
    pub revocation: Option<Arc<dyn CertificateRevocation>>,
    /// Locale policy and embedded catalogues (`GET /api/i18n`, Desk
    /// negotiation). The CLI replaces the English-only default before boot.
    pub i18n: I18n,
}

impl ServerSettings {
    pub fn new(
        host: String,
        port: u16,
        roles: ServerRoles,
        database: DatabaseConfig,
        config: Config,
    ) -> Self {
        Self {
            host,
            port,
            roles,
            database,
            config,
            // Secure by default: CORS is opt-in and no wildcard origins are
            // preloaded. Callers must add origins explicitly.
            enable_cors: false,
            cors_origins: Vec::new(),
            request_timeout_secs: 60,
            body_limit_mb: 50,
            realtime: RealtimeConfig::default(),
            #[cfg(feature = "wasm")]
            plugins: None,
            pool: None,
            desk_css_dev: None,
            dev: false,
            policy_path: None,
            revocation: None,
            i18n: I18n::default(),
        }
    }

    /// Validates the settings before the server starts serving.
    pub(super) fn validate(&self) -> Result<()> {
        if self.roles.is_empty() {
            anyhow::bail!(
                "no roles enabled: pass at least one of --api/--web/--realtime/--worker/--scheduler or --all"
            );
        }

        if self.enable_cors {
            if self.cors_origins.is_empty() {
                warn!(
                    "CORS is enabled but no origins are configured; no cross-origin requests will be allowed"
                );
            } else if let Some(wild) = self.cors_origins.iter().find(|o| o.trim() == "*") {
                // The router refuses '*'; only warn here so the
                // operator understands why their origin list is inert.
                warn!(
                    "CORS origin '{wild}' is ignored (wildcard allow-list not supported); list explicit origins"
                );
            }
        }

        Ok(())
    }
}
