//! Dev-only Desk operator so the login works out of the box.

use anyhow::Result;
use tracing::{info, warn};

/// Create a usable operator for the Desk login during development. Idempotent:
/// skipped once a matching account exists or the users table already has rows.
pub(super) fn seed_dev_admin(
    runtime_handle: &tokio::runtime::Runtime,
    pool: &ferris_db::Db,
) -> Result<()> {
    let store = ferris_auth::UserStore::new(pool.clone());
    let outcome = runtime_handle.block_on(store.ensure_admin(
        "admin@local",
        "ferrisAdmin123",
        &["System Manager".to_string()],
    ));
    match outcome {
        Ok(user) => {
            info!(
                email = %user.email,
                password = "ferrisAdmin123",
                "Desk login seeded — change it for anything beyond local dev"
            );
            Ok(())
        }
        Err(e) => {
            warn!(
                error = %e,
                "skipped auto-seeding a Desk operator (see error)"
            );
            Ok(())
        }
    }
}
