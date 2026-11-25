//! What a config with no file (or a partial file) starts from.

use ferris_core::{
    EmailConfig, FeatureFlags, JobsConfig, RealtimeConfig, SchedulerConfig, SearchConfig,
};
use std::collections::HashMap;

use super::auth::AuthConfig;
use super::cli::CliSettings;
use super::database::DatabaseConfig;
use super::files::FilesConfig;
use super::i18n::I18nConfig;
use super::project::ProjectConfig;
use super::redis::RedisConfig;
use super::server::{ServerConfig, default_body_limit_mb};
use super::types::{CliConfig, ConfigSource};

/// Placeholder values that ship with `CliConfig::default()`. The database
/// defaults below are only bootstrap examples for a local dev database.
pub(super) const DEFAULT_DB_USER: &str = "ferris";
pub(super) const DEFAULT_DB_PASSWORD: &str = "ferris";

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            project: ProjectConfig {
                name: "ferris-app".to_string(),
                version: "0.1.0".to_string(),
                description: "A Ferris application".to_string(),
                authors: vec!["Your Name <you@example.com>".to_string()],
                license: "MIT".to_string(),
                repository: None,
            },
            database: DatabaseConfig {
                r#type: "postgres".to_string(),
                host: "localhost".to_string(),
                port: 5432,
                name: "ferris_dev".to_string(),
                user: DEFAULT_DB_USER.to_string(),
                password: DEFAULT_DB_PASSWORD.to_string(),
                max_connections: 20,
                ssl_mode: "prefer".to_string(),
            },
            redis: RedisConfig {
                url: "redis://localhost:6379".to_string(),
                max_connections: 20,
            },
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 8000,
                workers: std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(1),
                request_timeout_secs: 60,
                body_limit_mb: default_body_limit_mb(),
                roles: vec!["all".to_string()],
                enable_cors: false,
                cors_origins: Vec::new(),
                tls: None,
                pki: None,
                external_url: None,
            },
            auth: AuthConfig {
                password_min_length: 8,
                password_require_uppercase: true,
                password_require_lowercase: true,
                password_require_numbers: true,
                password_require_symbols: false,
                session_idle_timeout_minutes: 30,
                session_timeout_minutes: 60,
                max_login_attempts: 5,
                lockout_duration_minutes: 15,
                max_sessions_per_user: 5,
                mfa_issuer: "Ferris".to_string(),
                require_email_verification: false,
                oauth: ferris_core::OAuthConfig::default(),
            },
            files: FilesConfig::default(),
            email: EmailConfig::default(),
            search: SearchConfig::default(),
            realtime: RealtimeConfig::default(),
            jobs: JobsConfig::default(),
            scheduler: SchedulerConfig::default(),
            features: FeatureFlags::default(),
            apps: ferris_core::AppsConfig::default(),
            secrets: ferris_core::SecretsConfig::default(),
            i18n: I18nConfig::default(),
            cli: CliSettings::default(),
            custom: HashMap::new(),
            source: ConfigSource::default(),
        }
    }
}
