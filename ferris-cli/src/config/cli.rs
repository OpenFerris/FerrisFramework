//! `[cli]` section: CLI-specific switches and feature unlocks.

use serde::{Deserialize, Serialize};

/// CLI-specific configuration and feature unlocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliSettings {
    /// Allow features that require access to the framework source tree
    /// (frontend hot-reload, live desk.css generation, etc.).
    #[serde(default)]
    pub unlock_source: bool,
}

impl Default for CliSettings {
    fn default() -> Self {
        Self {
            unlock_source: false,
        }
    }
}
