//! CLI Configuration
//!
//! One module per config section; this module only declares them and re-exports
//! the public names, so every `crate::config::Thing` path keeps resolving.

mod auth;
mod cli;
mod database;
mod defaults;
mod files;
mod i18n;
mod load;
mod project;
mod redis;
mod server;
mod types;

pub use auth::AuthConfig;
pub use cli::CliSettings;
pub use database::DatabaseConfig;
pub use files::FilesConfig;
pub use i18n::I18nConfig;
pub use project::ProjectConfig;
pub use redis::{RedisConfig, redact_url};
pub use server::ServerConfig;
pub use types::{CliConfig, ConfigSource};

pub(crate) use load::merge_toml;

#[cfg(test)]
mod tests;
