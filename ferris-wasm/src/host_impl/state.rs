//! The runtime's store type for the real host.

use super::RealHostApi;
use crate::host::HostState;
use ferris_db::Db;
use std::sync::Arc;

/// The real host as the runtime's store type.
///
/// This is the one state type plugins are instantiated against: a single
/// [`HostState`] whose [`HostApi`](crate::host::HostApi) is a [`RealHostApi`]. All WIT import
/// bridging lives on `HostState`; production hosts construct one with
/// [`HostState::for_pool`].
pub type RealHostState = HostState;

impl HostState {
    /// A real host wired to the framework services over a database pool.
    #[must_use]
    pub fn for_pool(pool: Db) -> Self {
        Self::new(Arc::new(RealHostApi::new(pool)))
    }
}

impl Default for HostState {
    /// An in-memory SQLite-backed host, used by tests and tools.
    fn default() -> Self {
        let pool = crate::setup::block_on(async {
            ferris_db::create_pool(&ferris_core::DatabaseConfig {
                r#type: ferris_core::DatabaseType::SQLite,
                host: "localhost".into(),
                port: 0,
                name: ":memory:".into(),
                user: String::new(),
                password: String::new(),
                max_connections: 5,
                min_connections: 1,
                connect_timeout_secs: 5,
                idle_timeout_secs: 300,
                max_lifetime_secs: 3600,
                ssl_mode: ferris_core::SslMode::Disable,
                schema: None,
            })
            .await
            .unwrap()
        });
        Self::for_pool(pool)
    }
}
