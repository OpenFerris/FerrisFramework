//! Migration command.
//!
//! Runs real schema migrations in-process using [`ferris_schema::MigrationRunner`]
//! against the database configured in `config/ferris.toml`. No shell-outs to
//! separate migration binaries.

use super::db_map::core_database;
use super::*;
use crate::config::CliConfig;
use tracing::{error, info};

pub fn run(args: super::MigrateArgs, config: &CliConfig) -> Result<()> {
    let rt = crate::runtime::block_on_runtime(None)?;

    rt.block_on(async {
        let database = core_database(&config.database, config.database.name.clone())?;

        let pool = ferris_db::create_pool(&database).await?;

        // The runner takes the dialect from the pool it will migrate, so the
        // two cannot disagree. A doctype that cannot be turned into SQL is an
        // error: dropping the result here used to migrate without that table
        // and report success.
        let mut runner = ferris_schema::MigrationRunner::for_db(&pool);
        runner.add_schema_migrations()?;

        if args.dry_run {
            let pending = runner.pending(&pool).await?;
            if pending.is_empty() {
                info!("No pending migrations.");
            } else {
                info!("Pending migrations (dry run, not applied):");
                for m in &pending {
                    println!("  {:>9}  {}", m.version, m.name);
                }
                info!("{} migration(s) pending.", pending.len());
            }
            return Ok(());
        }

        if args.force {
            info!("Running migrations with --force (checksum drift downgraded to warnings)...");
        } else {
            info!("Running migrations...");
        }

        let results = if args.force {
            runner.run_forced(&pool, args.target).await?
        } else {
            runner.run(&pool, args.target).await?
        };

        if results.is_empty() {
            info!("No pending migrations to apply.");
        }
        for result in &results {
            if result.success {
                info!(
                    "  ✓ {} ({}) in {}ms",
                    result.version, result.name, result.execution_time_ms
                );
            } else {
                error!(
                    "  ✗ {} ({}) failed: {}",
                    result.version,
                    result.name,
                    result.error.as_deref().unwrap_or("unknown error")
                );
            }
        }

        let failed = results.iter().filter(|r| !r.success).count();
        if failed > 0 {
            return Err(anyhow::anyhow!("{failed} migration(s) failed"));
        }

        // The app's own copy of its customizations wins at deploy time, the way
        // Frappe applies fixtures on migrate. This is where an exported world
        // becomes effective metadata again on a fresh database; `serve` does not
        // do it so a Desk edit made after the last export is never silently
        // reverted by a restart.
        let plugins = super::plugin_world::plugins_dir(&config.source.project_dir, &config.apps);
        let applied = super::plugin_world::apply_all(&pool, &plugins, &config.apps.enabled).await?;
        if applied > 0 {
            info!(
                "Applied {applied} plugin customization(s) from {}.",
                plugins.display()
            );
        }

        Ok(())
    })
}
