//! `[redis]` section, plus the URL redaction shared with `ferris config show`.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    pub url: String,
    pub max_connections: u32,
}

impl fmt::Debug for RedisConfig {
    /// N3: a Redis URL can embed a password (`redis://user:pass@host`);
    /// the derived `Debug` printed it in full.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedisConfig")
            .field("url", &redact_url(&self.url))
            .field("max_connections", &self.max_connections)
            .finish()
    }
}

/// Redact the password component of a `scheme://[user:]pass@host` URL without
/// pulling in a URL parser. Everything between the last `:` before the first
/// `@` and the `@` itself is the secret.
pub fn redact_url(url: &str) -> String {
    match url.split_once('@') {
        Some((userinfo, rest)) => {
            if let Some(colon) = userinfo.rfind(':') {
                format!("{}<redacted>@{}", &userinfo[..=colon], rest)
            } else {
                url.to_string()
            }
        }
        None => url.to_string(),
    }
}
