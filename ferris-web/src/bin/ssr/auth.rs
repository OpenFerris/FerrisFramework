//! Auth wiring and boot helpers: the ambient-user layer, the SSR desk context,
//! the health probe, the dev admin seed and the env-driven DB config.

use std::env;

use anyhow::{Result, anyhow};
use axum::{
    Json,
    extract::{FromRequestParts, Request, State},
    middleware,
    response::Response,
};
use ferris_auth::middleware::AuthenticatedUser;
use ferris_auth::{AuthState, UserStore};
use ferris_core::{DatabaseConfig, DatabaseType, SslMode};
use ferris_db::Db;
use leptos::prelude::*;
use tracing::{info, warn};

/// Runs an authenticated request with the resolved user scoped as the ambient
/// engine user so `DocStore`/typed CRUD authorises as the actual caller. Must
/// sit inside the `require_auth` layer (fails closed: 401 when unauthenticated).
pub(super) async fn bind_ambient_user(
    State(auth): State<AuthState>,
    request: Request,
    next: middleware::Next,
) -> Result<Response, Response> {
    let (mut parts, body) = request.into_parts();
    let user = AuthenticatedUser::from_request_parts(&mut parts, &auth).await?;
    let request = Request::from_parts(parts, body);
    Ok(ferris_document::with_user(Some(user.0), async move { next.run(request).await }).await)
}

/// Provide a Guest user and the data pool as SSR context for the Desk.
pub(super) fn provide_desk_context(pool: Db) {
    use ferris_auth::User;
    provide_context(User::guest());
    provide_context(pool);
}

/// JSON health probe for supervisors/liveness checks.
pub(super) async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "ferris-web-ssr" }))
}

/// Seeder for local development, so `admin@local / ferrisAdmin123` can log into
/// the desk against a fresh database.
pub(super) async fn seed_dev_admin(pool: &Db) {
    let store = UserStore::new(pool.clone());
    match store
        .ensure_admin(
            "admin@local",
            "ferrisAdmin123",
            &["System Manager".to_string()],
        )
        .await
    {
        Ok(user) => {
            info!(email = %user.email, "Desk login seeded (admin@local / ferrisAdmin123)")
        }
        Err(e) => warn!(error = %e, "desk operator already present; skipping seed"),
    }
}

/// Rebuilds the `DatabaseConfig` the supervisor forwarded via env vars.
pub(super) fn database_config_from_env() -> Result<DatabaseConfig> {
    let r#type = match env::var("FERRIS_DEV_DB_TYPE").as_deref() {
        Ok("postgres") => DatabaseType::Postgres,
        Ok("mysql") => DatabaseType::MySQL,
        Ok("sqlite") => DatabaseType::SQLite,
        other => {
            return Err(anyhow!(
                "FERRIS_DEV_DB_TYPE missing or unsupported ({other:?}): run `ferris dev --hot-reload front` inside a Ferris project"
            ));
        }
    };
    let ssl_mode = match env::var("FERRIS_DEV_DB_SSL_MODE").as_deref() {
        Ok("disable") => SslMode::Disable,
        Ok("allow") => SslMode::Allow,
        Ok("prefer") => SslMode::Prefer,
        Ok("require") => SslMode::Require,
        Ok("verify_ca") => SslMode::VerifyCa,
        Ok("verify_full") => SslMode::VerifyFull,
        Ok(other) => {
            return Err(anyhow!(
                "unsupported FERRIS_DEV_DB_SSL_MODE {other:?} (expected disable/allow/prefer/require/verify_ca/verify_full)"
            ));
        }
        Err(_) => SslMode::Disable,
    };
    Ok(DatabaseConfig {
        r#type,
        host: env::var("FERRIS_DEV_DB_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
        port: env::var("FERRIS_DEV_DB_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3306),
        name: env::var("FERRIS_DEV_DB_NAME").unwrap_or_default(),
        user: env::var("FERRIS_DEV_DB_USER").unwrap_or_default(),
        password: env::var("FERRIS_DEV_DB_PASSWORD").unwrap_or_default(),
        max_connections: env::var("FERRIS_DEV_DB_MAX_CONNS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10),
        min_connections: 5,
        connect_timeout_secs: 30,
        idle_timeout_secs: 300,
        max_lifetime_secs: 3600,
        ssl_mode,
        schema: None,
    })
}
