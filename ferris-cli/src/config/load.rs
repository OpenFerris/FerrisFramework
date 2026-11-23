//! Loading: file discovery, `toml` merge, environment overrides and the
//! fail-closed checks that run on every loaded config.

use ferris_core::{Error, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::warn;

use super::database::DatabaseConfig;
use super::defaults::{DEFAULT_DB_PASSWORD, DEFAULT_DB_USER};
use super::types::{CliConfig, ConfigSource};

impl CliConfig {
    pub fn load(config_file: Option<&str>, project_dir: Option<&str>) -> Result<Self> {
        // An explicit `--config` must point at an existing file: silently
        // falling back to defaults would hide typos and bad deploys.
        let explicit = config_file.is_some();
        let invocation = match project_dir {
            Some(dir) => PathBuf::from(dir),
            None => std::env::current_dir()?,
        };
        let source = if let Some(file) = config_file {
            let config_path = PathBuf::from(file);
            let bench_root = config_path
                .parent()
                .filter(|parent| parent.file_name().is_some_and(|name| name == "config"))
                .and_then(Path::parent)
                .map_or_else(|| invocation.clone(), Path::to_path_buf);
            ConfigSource {
                config_path,
                bench_root,
                project_dir: invocation,
            }
        } else {
            let (bench_root, config_path) = match find_bench_root(&invocation) {
                Some((root, path)) => (root, path),
                None => (
                    invocation.clone(),
                    invocation.join("config").join("ferris.toml"),
                ),
            };
            ConfigSource {
                config_path,
                bench_root,
                project_dir: invocation,
            }
        };
        let config_path = source.config_path.clone();

        let mut config = if config_path.exists() {
            load_from_file(&config_path)?
        } else if explicit {
            return Err(Error::Config(ferris_core::ConfigError::NotFound(
                config_path.display().to_string(),
            )));
        } else {
            // No file and no explicit request: this is a valid, purely
            // env-driven setup. Start from defaults.
            Self::default()
        };

        // Env overrides apply regardless of whether a file was loaded, so
        // containerized deployments can drive config entirely via env vars.
        config.apply_env_overrides();
        config.reject_insecure_jwt_secret()?;
        config.warn_insecure_defaults();
        config.source = source;

        // One bench, one SQLite database. `ferris dev --app <name>` moves into
        // `apps/<name>/` before opening the pool, so a relative `database.name`
        // would create a different file per command (and a second one for
        // `ferris migrate` run from the root). Resolving it against the bench
        // root keeps every command on the same file; a standalone app's root is
        // its own directory, so nothing changes there.
        if config.database.r#type.eq_ignore_ascii_case("sqlite")
            && !config.database.name.is_empty()
            && Path::new(&config.database.name).is_relative()
        {
            config.database.name = config
                .source
                .bench_root
                .join(&config.database.name)
                .display()
                .to_string();
        }

        Ok(config)
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(url) = std::env::var("DATABASE_URL") {
            // Keep the raw value available for callers that prefer to hand it
            // straight to the pool.
            self.custom.insert(
                "database_url".to_string(),
                serde_json::Value::String(url.clone()),
            );

            if !apply_database_url(&url, &mut self.database) {
                warn!(
                    "DATABASE_URL is set but could not be applied to the structured \
                     database config (unsupported scheme or malformed); it is still \
                     available under `custom.database_url`"
                );
            }
        }

        if let Ok(redis_url) = std::env::var("REDIS_URL") {
            self.redis.url = redis_url;
        }

        if let Ok(jwt_secret) =
            std::env::var("FERRIS_JWT_SECRET").or_else(|_| std::env::var("JWT_SECRET"))
        {
            // An empty variable means "not set". `compose.yaml` forwards
            // `FERRIS_JWT_SECRET: ${FERRIS_JWT_SECRET:-}` so a developer who has
            // not exported a secret gets the fail-closed "not configured" error
            // instead of a process that signs tokens with an empty key.
            let jwt_secret = jwt_secret.trim();
            if !jwt_secret.is_empty() {
                self.auth.jwt_secret = Some(jwt_secret.to_string());
            }
        }

        // Secret-field key material. Both spellings are accepted: the flat one
        // is what a container sets next to `FERRIS_JWT_SECRET`, the nested one is
        // what `ferris_core::Config::load_from` (figment) reads, so a single
        // `FERRIS_SECRETS__KEY` works for both loaders.
        if let Ok(key) =
            std::env::var("FERRIS_SECRET_KEY").or_else(|_| std::env::var("FERRIS_SECRETS__KEY"))
        {
            self.secrets.key = Some(key);
        }
        if let Ok(previous) = std::env::var("FERRIS_SECRET_PREVIOUS_KEYS") {
            self.secrets.previous_keys = previous
                .split(',')
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_string)
                .collect();
        }

        if let Ok(host) = std::env::var("SERVER_HOST") {
            self.server.host = host;
        }

        if let Ok(port) = std::env::var("SERVER_PORT") {
            match port.parse::<u16>() {
                Ok(p) => self.server.port = p,
                Err(_) => warn!("SERVER_PORT={port:?} is not a valid u16; ignoring"),
            }
        }

        if let Ok(limit) = std::env::var("FERRIS_BODY_LIMIT_MB") {
            match limit.parse::<usize>() {
                Ok(mb) if mb > 0 => self.server.body_limit_mb = mb,
                Ok(_) => warn!("FERRIS_BODY_LIMIT_MB=0 is not a usable limit; ignoring"),
                Err(_) => warn!("FERRIS_BODY_LIMIT_MB={limit:?} is not a number; ignoring"),
            }
        }

        apply_bool_env("FERRIS_REALTIME", &mut self.features.realtime);
        apply_bool_env("FERRIS_JOBS", &mut self.features.jobs);
        apply_bool_env("FERRIS_SCHEDULER", &mut self.features.scheduler);
        apply_bool_env("FERRIS_EMAIL", &mut self.features.email);
        apply_bool_env("FERRIS_FILES", &mut self.features.files);
        apply_bool_env("FERRIS_PRINT", &mut self.features.print);
        apply_bool_env("FERRIS_REPORT", &mut self.features.report);
        apply_bool_env("FERRIS_SEARCH", &mut self.features.search);
        apply_bool_env("FERRIS_WEBHOOKS", &mut self.features.webhooks);
        if let Ok(v) = std::env::var("FERRIS_UNLOCK_SOURCE") {
            match v.parse::<bool>() {
                Ok(b) => self.cli.unlock_source = b,
                Err(_) => warn!("FERRIS_UNLOCK_SOURCE={v:?} is not a boolean; ignoring"),
            }
        }

        self.apply_tls_env_overrides();
        self.apply_pki_env_overrides();
    }

    /// TLS + mTLS overrides. Unlike the other server fields these are read from
    /// flat `FERRIS_TLS_*` / `FERRIS_MTLS_*` variables rather than the nested
    /// `[server.tls]` table, because certificates and a CA path are exactly the
    /// values a container injects as environment variables.
    ///
    /// A `[server.tls]` table from the config file is the base; environment
    /// values override it field by field. Setting just one of `FERRIS_TLS_CERT`
    /// / `FERRIS_TLS_KEY` still produces a config that fails loudly at boot
    /// (the missing file cannot be read) instead of silently disabling TLS.
    fn apply_tls_env_overrides(&mut self) {
        let cert = std::env::var("FERRIS_TLS_CERT").ok();
        let key = std::env::var("FERRIS_TLS_KEY").ok();

        if cert.is_none() && key.is_none() && self.server.tls.is_none() {
            return;
        }

        let tls = self
            .server
            .tls
            .get_or_insert_with(ferris_core::TlsConfig::default);
        if let Some(cert) = cert {
            tls.cert_path = PathBuf::from(cert);
        }
        if let Some(key) = key {
            tls.key_path = PathBuf::from(key);
        }
        if let Ok(ca) = std::env::var("FERRIS_MTLS_CLIENT_CA") {
            tls.client_ca_path = Some(PathBuf::from(ca));
        }
        if let Ok(v) = std::env::var("FERRIS_MTLS_REQUIRE") {
            match v.parse::<bool>() {
                Ok(b) => tls.require_client_cert = b,
                Err(_) => warn!("FERRIS_MTLS_REQUIRE={v:?} is not a boolean; ignoring"),
            }
        }
        if let Ok(v) = std::env::var("FERRIS_TLS_MIN_VERSION") {
            tls.min_version = Some(v);
        }
    }

    /// Internal-CA overrides (`FERRIS_CA_*`).
    ///
    /// Both the certificate and its key are required for a usable CA: setting
    /// only one produces a config that fails loudly at boot (the missing file
    /// cannot be read) rather than shipping a half-configured authority. A
    /// `[server.pki]` table from the config file is the base and the
    /// environment overrides it field by field.
    fn apply_pki_env_overrides(&mut self) {
        let cert = std::env::var("FERRIS_CA_CERT").ok();
        let key = std::env::var("FERRIS_CA_KEY").ok();
        let ttl = std::env::var("FERRIS_CA_TTL_SECONDS").ok();

        if cert.is_none() && key.is_none() && ttl.is_none() && self.server.pki.is_none() {
            return;
        }

        let pki = self
            .server
            .pki
            .get_or_insert_with(|| ferris_core::PkiConfig {
                ca_cert_path: PathBuf::new(),
                ca_key_path: PathBuf::new(),
                default_ttl_seconds: 90 * 24 * 60 * 60,
            });
        if let Some(cert) = cert {
            pki.ca_cert_path = PathBuf::from(cert);
        }
        if let Some(key) = key {
            pki.ca_key_path = PathBuf::from(key);
        }
        if let Some(v) = ttl {
            if let Ok(seconds) = v.parse::<u64>() {
                pki.default_ttl_seconds = seconds;
            } else {
                warn!("FERRIS_CA_TTL_SECONDS={v:?} is not a u64; ignoring");
            }
        }
    }

    /// Placeholder JWT secrets are a hard error, never a warning: a config
    /// carrying one would ship tokens anyone can forge.
    ///
    /// Two published shapes are rejected. `change-me-in-production` is the old
    /// scaffold default; `dev-only-change-me-*` is the value `compose.yaml` used
    /// to inject into every developer container, and it was accepted by `serve`
    /// for as long as it lived in this file. `ferris dev` is unaffected: it
    /// generates its own ephemeral secret when none is configured, and never
    /// reaches this branch with a placeholder unless the operator exported one
    /// on purpose — which is still a public signing key.
    pub(super) fn reject_insecure_jwt_secret(&self) -> Result<()> {
        const PLACEHOLDER: &str = "change-me-in-production";
        const DEV_PREFIX: &str = "dev-only-change-me";

        let Some(secret) = self.auth.jwt_secret.as_deref() else {
            return Ok(());
        };
        // Never echo the value: the error travels to logs and to `ferris doctor`.
        let placeholder = if secret == PLACEHOLDER {
            "the old 'change-me-in-production' placeholder"
        } else if secret.starts_with(DEV_PREFIX) {
            "the 'dev-only-change-me-…' development placeholder"
        } else {
            return Ok(());
        };

        Err(Error::Config(ferris_core::ConfigError::Invalid {
            key: "auth.jwt_secret".to_string(),
            msg: format!(
                "is {placeholder}; it would sign sessions with a key anyone can read. \
                 Generate a real secret with `openssl rand -base64 48` and set it as \
                 FERRIS_JWT_SECRET (or [auth] jwt_secret)"
            ),
        }))
    }

    /// Resolve the JWT secret required to sign/validate tokens, failing loudly
    /// when it is not configured. Part of the fail-closed posture: commands
    /// that touch auth never run anonymous.
    pub fn require_jwt_secret(&self) -> Result<&str> {
        self.auth.jwt_secret.as_deref().ok_or_else(|| {
            Error::Config(ferris_core::ConfigError::Missing(
                "auth.jwt_secret; set FERRIS_JWT_SECRET/JWT_SECRET".to_string(),
            ))
        })
    }

    /// Emits a `warn!` for every value that still matches its placeholder
    /// default, so operators notice before shipping to production.
    fn warn_insecure_defaults(&self) {
        if self.database.user == DEFAULT_DB_USER && self.database.password == DEFAULT_DB_PASSWORD {
            warn!(
                "database credentials are still the example defaults \
                 (user={DEFAULT_DB_USER:?}, password=<default>); do not use in production"
            );
        }
    }
}

/// Reads a TOML config file and merges it over the compiled-in defaults so
/// partial configs remain valid.
/// Walk up from `start` looking for the bench root (`<dir>/config/ferris.toml`).
fn find_bench_root(start: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let path = dir.join("config").join("ferris.toml");
        if path.is_file() {
            return Some((dir.to_path_buf(), path));
        }
        current = dir.parent();
    }
    None
}

fn load_from_file(path: &Path) -> Result<CliConfig> {
    let content = fs::read_to_string(path).map_err(|e| {
        Error::Config(ferris_core::ConfigError::Invalid {
            key: path.display().to_string(),
            msg: format!("failed to read: {e}"),
        })
    })?;

    let file_value: toml::Value = toml::from_str(&content).map_err(|e| {
        Error::Config(ferris_core::ConfigError::Invalid {
            key: path.display().to_string(),
            msg: format!("failed to parse: {e}"),
        })
    })?;

    let mut merged = toml::Value::try_from(CliConfig::default()).map_err(|e| {
        Error::Config(ferris_core::ConfigError::Invalid {
            key: "config".to_string(),
            msg: format!("failed to build default config: {e}"),
        })
    })?;
    merge_toml(&mut merged, file_value);

    merged.try_into().map_err(|e| {
        Error::Config(ferris_core::ConfigError::Invalid {
            key: "config".to_string(),
            msg: format!("invalid config: {e}"),
        })
    })
}

fn apply_bool_env(var: &str, target: &mut bool) {
    if let Ok(v) = std::env::var(var) {
        match v.parse::<bool>() {
            Ok(b) => *target = b,
            Err(_) => warn!("{var}={v:?} is not a boolean; ignoring"),
        }
    }
}

/// Best-effort parse of `DATABASE_URL` into the structured `DatabaseConfig`.
///
/// Handles `postgres://[user[:pass]@]host[:port][/dbname]` and the
/// `postgresql://` alias. Returns `false` for any other scheme (e.g. `sqlite://`)
/// or on malformed input, so the caller can decide how to surface it.
pub(super) fn apply_database_url(url: &str, db: &mut DatabaseConfig) -> bool {
    let (scheme, rest) = match url.split_once("://") {
        Some(p) => p,
        None => return false,
    };
    if !scheme.eq_ignore_ascii_case("postgres") && !scheme.eq_ignore_ascii_case("postgresql") {
        return false;
    }

    let (userinfo, hostpath) = match rest.split_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, rest),
    };

    let (hostport, dbname) = match hostpath.split_once('/') {
        Some((h, d)) => (h, Some(d)),
        None => (hostpath, None),
    };

    if let Some(userinfo) = userinfo {
        match userinfo.split_once(':') {
            Some((u, p)) => {
                db.user = u.to_string();
                db.password = p.to_string();
            }
            None => db.user = userinfo.to_string(),
        }
    }

    let (host, port) = match hostport.split_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(p) => (h.to_string(), Some(p)),
            Err(_) => return false,
        },
        None => (hostport.to_string(), None),
    };
    db.host = host;
    if let Some(p) = port {
        db.port = p;
    }

    if let Some(name) = dbname {
        // Drop the query string (`?sslmode=require` etc.); those knobs are
        // not modeled here on purpose.
        let name = name.split('?').next().unwrap_or(name);
        if !name.is_empty() {
            db.name = name.to_string();
        }
    }

    db.r#type = "postgres".to_string();
    true
}

/// Recursively merge `overlay` into `base` (tables are merged key by key,
/// scalars/arrays replace).
pub(crate) fn merge_toml(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                merge_toml(
                    base.entry(key)
                        .or_insert(toml::Value::Table(Default::default())),
                    value,
                );
            }
        }
        (base, overlay) => *base = overlay,
    }
}

/// Regression tests for the published JWT placeholders. They sit next to the
/// guard instead of `config/tests.rs` because that file is outside this
/// change's file list; move them there when the module is next touched.
#[cfg(test)]
mod jwt_placeholder_tests {
    use super::*;

    fn with_secret(secret: &str) -> CliConfig {
        let mut config = CliConfig::default();
        config.auth.jwt_secret = Some(secret.to_string());
        config
    }

    #[test]
    fn rejects_the_compose_development_placeholder() {
        let config = with_secret("dev-only-change-me-32-characters");
        let error = config
            .reject_insecure_jwt_secret()
            .expect_err("a published development secret must not reach `serve`");
        let message = error.to_string();

        assert!(
            message.contains("openssl rand"),
            "the error must say how to generate a real secret: {message}"
        );
        assert!(
            !message.contains("32-characters"),
            "the error must not echo the rejected secret: {message}"
        );
    }

    #[test]
    fn rejects_the_bare_development_prefix() {
        let error = with_secret("dev-only-change-me")
            .reject_insecure_jwt_secret()
            .expect_err("the prefix alone is still the public default");
        assert!(
            error.to_string().contains("development placeholder"),
            "{error}"
        );
    }

    #[test]
    fn accepts_a_generated_secret() {
        // Whatever `openssl rand -base64 48` prints.
        let generated = "Zt7Kp2Qm9Xr4Lb8Vc1Nd6Hf3Jw5Sy0AuEeIiOoPpQqRrSsTt";
        assert!(
            with_secret(generated).reject_insecure_jwt_secret().is_ok(),
            "a real secret must keep loading"
        );
    }

    #[test]
    fn only_the_prefix_is_rejected_not_the_substring() {
        // `starts_with`, not `contains`: an operator-chosen secret that merely
        // mentions the placeholder is theirs, not the published default.
        assert!(
            with_secret("my-dev-only-change-me-but-not-public-0123456789")
                .reject_insecure_jwt_secret()
                .is_ok()
        );
    }
}
