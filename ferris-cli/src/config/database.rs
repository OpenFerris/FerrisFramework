//! `[database]` section.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub r#type: String,
    pub host: String,
    pub port: u16,
    pub name: String,
    pub user: String,
    pub password: String,
    pub max_connections: u32,
    pub ssl_mode: String,
}

impl fmt::Debug for DatabaseConfig {
    /// N3: `#[derive(Debug)]` dumped `password` in plaintext — a panic
    /// path or a stray `{:?}` in a log leaked the database credential, while
    /// `ferris config show` redacted it at display time. The type itself must
    /// be safe to formats.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("type", &self.r#type)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("name", &self.name)
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .field("max_connections", &self.max_connections)
            .field("ssl_mode", &self.ssl_mode)
            .finish()
    }
}
