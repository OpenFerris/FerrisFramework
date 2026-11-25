//! Shared fixtures: pools, principals and a host wired to a temporary store.

use super::super::RealHostApi;
use super::super::http::HttpPolicy;
use ferris_auth::user::User;
use ferris_document::engine::DocumentEngine;
use ferris_schema::{DoctypeMeta, FieldMeta};
use parking_lot::Mutex;
use std::collections::HashMap;

/// The ad-hoc `Probe` and `Notification` metadata these tests use.
///
/// The document tables are columnar; without a declaration a body field has no
/// column and is dropped. Registering the fixtures is what lets the tests
/// exercise the same path a real app takes.
fn register_fixtures() {
    ferris_schema::register_doctype(DoctypeMeta {
        name: "Probe".to_string(),
        fields: vec![
            FieldMeta::data("title"),
            FieldMeta::data("region"),
            FieldMeta::data("amount"),
        ],
        ..Default::default()
    });
    ferris_schema::register_doctype(DoctypeMeta {
        name: "Notification".to_string(),
        fields: vec![
            FieldMeta::data("user"),
            FieldMeta::data("notification_type"),
            FieldMeta::data("title"),
            FieldMeta::data("description"),
            FieldMeta::data("route"),
            FieldMeta::check("is_read"),
        ],
        ..Default::default()
    });
}

pub(super) async fn memory_pool() -> ferris_db::Db {
    register_fixtures();
    ferris_db::create_pool(&ferris_core::DatabaseConfig {
        r#type: ferris_core::DatabaseType::SQLite,
        host: "localhost".into(),
        port: 0,
        name: ":memory:".into(),
        user: String::new(),
        password: String::new(),
        max_connections: 1,
        min_connections: 1,
        connect_timeout_secs: 5,
        idle_timeout_secs: 300,
        max_lifetime_secs: 3600,
        ssl_mode: ferris_core::SslMode::Disable,
        schema: None,
    })
    .await
    .unwrap()
}

/// A `System Manager` principal: the engine's `Create`/`Read` checks need a
/// real role, not the anonymous guest.
pub(super) fn admin() -> User {
    let mut user = User::system_user("admin@local".to_string(), "Admin".to_string());
    user.roles = vec!["System Manager".to_string()];
    user
}

pub(super) fn test_host(pool: ferris_db::Db) -> RealHostApi {
    RealHostApi {
        engine: DocumentEngine::new(pool.clone()),
        pool,
        job_queue: None,
        scheduler: Mutex::new(None),
        streams: Mutex::new(HashMap::new()),
        stream_buffers: Mutex::new(HashMap::new()),
        current_user: Mutex::new(Some(admin())),
        certificate_service: Mutex::new(false),
        http: HttpPolicy::from_env(),
    }
}

/// A plain (non-admin) principal with a **stable** `id` equal to its email:
/// `User::system_user` otherwise mints a random UUID, which would never
/// match the `created_by` the blob test helpers write, so the owner-read
/// ACL assertion would fail for the wrong reason.
pub(super) fn plain_user(id: &str) -> User {
    let mut user = User::system_user(id.to_string(), id.to_string());
    user.id = id.to_string();
    user
}
