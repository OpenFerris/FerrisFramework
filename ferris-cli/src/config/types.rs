//! The top-level config type and where it was read from.

use ferris_core::{
    EmailConfig, FeatureFlags, JobsConfig, RealtimeConfig, SchedulerConfig, SearchConfig,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use super::auth::AuthConfig;
use super::cli::CliSettings;
use super::database::DatabaseConfig;
use super::files::FilesConfig;
use super::i18n::I18nConfig;
use super::project::ProjectConfig;
use super::redis::RedisConfig;
use super::server::ServerConfig;

/// CLI configuration
#[derive(Clone, Serialize, Deserialize)]
pub struct CliConfig {
    pub project: ProjectConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub server: ServerConfig,
    pub auth: AuthConfig,
    /// Files backend. Defaults match [`ferris_core::FilesConfig`]; a section
    /// written before the S3/Azure/GCS fields existed keeps parsing because the
    /// missing keys take their defaults.
    #[serde(default)]
    pub files: FilesConfig,
    /// Outbound email accounts and queue policy (`[email]`). Previously absent
    /// from the CLI, which is why `serve` always passed an empty
    /// `CoreConfig.email` and no account could send.
    #[serde(default)]
    pub email: EmailConfig,
    /// Search settings (`[search]`). Consumed by the `/api/search` route.
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub realtime: RealtimeConfig,
    #[serde(default)]
    pub jobs: JobsConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    /// Install-wide feature switches (`[features]`).
    #[serde(default)]
    pub features: FeatureFlags,
    /// WASM apps the server publishes at boot (`[apps]`). Read by
    /// `ferris serve`/`run` before `Server::run`, so `/plugins` and
    /// `/api/method/<name>` exist from the first request.
    #[serde(default)]
    pub apps: ferris_core::AppsConfig,
    /// Key material for recoverable `Secret` fields (`[secrets]`). Kept on the
    /// CLI config too so `ferris doctor`/`shell`/`migrate` install the same
    /// keyring the server does, and so an operator can see it redacted.
    #[serde(default)]
    pub secrets: ferris_core::SecretsConfig,
    /// Locale policy (`[i18n]`): default locale and advertised set.
    #[serde(default)]
    pub i18n: I18nConfig,
    /// CLI-specific settings and feature unlocks.
    #[serde(default)]
    pub cli: CliSettings,
    pub custom: HashMap<String, serde_json::Value>,

    /// Where this config was read from. Not part of the config file: it is how
    /// the file was found, which `ferris config` needs in order to edit the same
    /// one `ferris` loaded instead of guessing `current_dir()/config/ferris.toml`
    /// and silently ignoring `--config` and `--dir`.
    #[serde(skip)]
    pub source: ConfigSource,
}

impl fmt::Debug for CliConfig {
    /// N3: the derived `Debug` printed every field verbatim, including
    /// secrets only the leaf types redact. This impl goes one step further and
    /// redacts secret-named keys inside the free-form `custom` map too, so no
    /// `{:?}` — in an error, a panic or a log — can ever print a credential.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let custom: HashMap<String, serde_json::Value> = self
            .custom
            .iter()
            .map(|(k, v)| {
                if is_secret_key(k) {
                    (
                        k.clone(),
                        serde_json::Value::String("<redacted>".to_string()),
                    )
                } else {
                    (k.clone(), v.clone())
                }
            })
            .collect();
        f.debug_struct("CliConfig")
            .field("project", &self.project)
            .field("database", &self.database)
            .field("redis", &self.redis)
            .field("server", &self.server)
            .field("auth", &self.auth)
            .field("files", &self.files)
            .field("email", &self.email)
            .field("search", &self.search)
            .field("realtime", &self.realtime)
            .field("jobs", &self.jobs)
            .field("scheduler", &self.scheduler)
            .field("features", &self.features)
            .field("apps", &self.apps)
            .field("secrets", &self.secrets)
            .field("i18n", &self.i18n)
            .field("cli", &self.cli)
            .field("custom", &custom)
            .field("source", &self.source)
            .finish()
    }
}

/// Key names whose value is a secret wherever they appear — including inside
/// the free-form `custom` map. Same list `ferris config show` uses to redact.
fn is_secret_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "password"
            | "passwd"
            | "secret"
            | "jwt_secret"
            | "token"
            | "access_token"
            | "refresh_token"
            | "api_key"
            | "private_key"
            | "signing_key"
    )
}

/// Resolved locations for a config, from `--config` and `--dir`.
#[derive(Debug, Clone)]
pub struct ConfigSource {
    /// The file `ferris config` reads and writes.
    pub config_path: PathBuf,
    /// Where the command was invoked (the app directory for `build`/`dev`).
    pub project_dir: PathBuf,
    /// The bench root: the directory holding `config/ferris.toml`, `apps/` and
    /// `sites/`, found by walking up from the invocation directory.
    pub bench_root: PathBuf,
}

impl Default for ConfigSource {
    fn default() -> Self {
        let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            config_path: project_dir.join("config").join("ferris.toml"),
            bench_root: project_dir.clone(),
            project_dir,
        }
    }
}
