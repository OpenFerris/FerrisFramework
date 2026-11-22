//! Tenant management command.
//!
//! Tenant configurations are persisted with [`TenantManager`] (one JSON file per
//! tenant). Creating a tenant also provisions its database pool and files
//! directory.

use super::db_map::core_database;
use super::*;
use crate::config::CliConfig;
use ferris_core::{TenantConfig, TenantManager};
use std::path::PathBuf;
use tracing::{error, info, warn};

pub fn run(args: super::TenantArgs, config: &CliConfig) -> Result<()> {
    match args.command {
        super::TenantCommand::Create {
            name,
            host,
            db_name,
            admin_email,
            admin_password,
        } => {
            let password = resolve_admin_password(admin_password)?;
            create_tenant(config, name, host, db_name, admin_email, password)
        }
        super::TenantCommand::List => list_tenants(config),
        super::TenantCommand::Info { id } => get_tenant(config, &id),
        super::TenantCommand::Delete { id, force } => delete_tenant(config, &id, force),
        super::TenantCommand::Migrate { id, target } => migrate_tenant(config, &id, target),
        super::TenantCommand::Enable { id } => set_enabled(config, &id, true),
        super::TenantCommand::Disable { id } => set_enabled(config, &id, false),
    }
}

/// Where this project's tenant store lives.
///
/// `TenantManager::default()` is `FERRIS_TENANTS_DIR`, else
/// `~/.config/ferris/tenants`: one directory for the whole machine. Every tenant
/// subcommand used it whatever `--dir` said, so `ferris --dir /srv/app tenant
/// list` listed the machine's tenants as if they were that project's, and
/// `tenant create` wrote its configuration outside the project it was pointed at
/// — the same "`--dir` does not reach the filesystem" bug as the rest of the
/// crate, one directory further out.
fn tenants_root(project_dir: &std::path::Path, override_dir: Option<&str>) -> PathBuf {
    match override_dir {
        Some(dir) => PathBuf::from(dir),
        None => project_dir.join("tenants"),
    }
}

/// The explicit override, when it names something.
///
/// An empty value is not a directory, and `TenantManager::default()` took it as
/// one, which made every tenant path relative to the cwd.
fn tenants_override() -> Option<String> {
    std::env::var("FERRIS_TENANTS_DIR")
        .ok()
        .filter(|dir| !dir.is_empty())
}

pub(in crate::commands) fn manager(config: &CliConfig) -> TenantManager {
    TenantManager::new(tenants_root(
        &config.source.project_dir,
        tenants_override().as_deref(),
    ))
}

/// Point this process at a tenant's data before the listener boots.
///
/// The deployment model is **one instance per tenant**: `ferris serve --tenant`
/// / `ferris run --tenant` rewrite the database, files root, Redis URL and the
/// app list here, and the server then runs as that tenant for its whole life.
/// Requests are not routed to a tenant by `Host` and the JWT `tenant` claim is
/// not compared — isolation comes from the process, its pool and its files
/// root, never from a per-request switch.
///
/// Pure on purpose: the merge is testable without opening a database.
pub(in crate::commands) fn apply_tenant(config: &mut CliConfig, tenant: &TenantConfig) {
    config.database.r#type = match tenant.database.r#type {
        ferris_core::DatabaseType::Postgres => "postgres",
        ferris_core::DatabaseType::MySQL => "mysql",
        ferris_core::DatabaseType::SQLite => "sqlite",
    }
    .to_string();
    config.database.host = tenant.database.host.clone();
    config.database.port = tenant.database.port;
    config.database.name = tenant.database.name.clone();
    config.database.user = tenant.database.user.clone();
    config.database.password = tenant.database.password.clone();
    config.database.max_connections = tenant.database.max_connections;
    config.database.ssl_mode = match tenant.database.ssl_mode {
        ferris_core::SslMode::Disable => "disable",
        ferris_core::SslMode::Allow => "allow",
        ferris_core::SslMode::Prefer => "prefer",
        ferris_core::SslMode::Require => "require",
        ferris_core::SslMode::VerifyCa => "verify-ca",
        ferris_core::SslMode::VerifyFull => "verify-full",
    }
    .to_string();
    config.redis.url = tenant.redis_url.clone();
    config.files.local_path = tenant.files_path.display().to_string();
    config.apps.enabled = tenant.enabled_apps.clone();
}

/// Resolve `--tenant <id>` and apply it to `config`.
///
/// A disabled tenant is refused rather than served: its configuration and data
/// stay on disk, but enabling it is an explicit `ferris tenant enable`.
pub(in crate::commands) fn apply_tenant_by_id(config: &mut CliConfig, id: &str) -> Result<()> {
    let tenant = manager(config).find(id)?;
    if !tenant.enabled {
        anyhow::bail!(
            "tenant '{}' is disabled; run `ferris tenant enable {}` first",
            tenant.name,
            tenant.id
        );
    }
    info!(
        "Serving tenant '{}' (id {}) from database '{}'",
        tenant.name, tenant.id, tenant.database.name
    );
    apply_tenant(config, &tenant);
    Ok(())
}

/// Where the admin password came from.
#[derive(Debug, PartialEq, Eq)]
enum PasswordSource {
    /// `--admin-password`, i.e. the process's own argv.
    Flag,
    /// `FERRIS_ADMIN_PASSWORD`.
    Environment,
    /// Asked for on the terminal, with the input masked.
    Prompt,
}

/// The admin password, from the flag, the environment or a prompt.
///
/// `--admin-password` was the only way in, which meant every provisioning script
/// put the tenant's admin password in its own argv, where `ps` shows it to every
/// user on the machine and it lands in the shell history. The flag still works and
/// now says what it costs; the environment variable does not have that problem,
/// and a terminal gets asked instead.
///
/// The environment value is a parameter rather than a read here so the precedence
/// can be tested without mutating the process's environment.
fn resolve_admin_password_with(
    from_flag: Option<String>,
    from_env: Option<String>,
    prompt: impl FnOnce() -> Result<String>,
) -> Result<(String, PasswordSource)> {
    let usable = |value: Option<String>| value.filter(|v| !v.is_empty());

    if let Some(password) = usable(from_flag) {
        warn!(
            "--admin-password is this process's argv, so `ps` shows it to every user on \
             this machine. FERRIS_ADMIN_PASSWORD does not."
        );
        return Ok((password, PasswordSource::Flag));
    }
    if let Some(password) = usable(from_env) {
        return Ok((password, PasswordSource::Environment));
    }
    let password = prompt()?;
    if password.is_empty() {
        anyhow::bail!("the admin password cannot be empty");
    }
    Ok((password, PasswordSource::Prompt))
}

fn resolve_admin_password(from_flag: Option<String>) -> Result<String> {
    let from_env = std::env::var("FERRIS_ADMIN_PASSWORD").ok();
    let (password, source) = resolve_admin_password_with(from_flag, from_env, || {
        use dialoguer::Password;
        Password::new()
            .with_prompt("Admin password")
            .interact()
            .map_err(anyhow::Error::from)
    })?;
    if source == PasswordSource::Prompt {
        info!("Read the admin password from the terminal.");
    }
    Ok(password)
}

fn slugify(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

fn create_tenant(
    cli: &CliConfig,
    name: String,
    host: String,
    db_name: Option<String>,
    admin_email: String,
    admin_password: String,
) -> Result<()> {
    let manager = manager(cli);
    // A duplicate host would otherwise be discovered *after* the admin user was
    // created in a database this tenant does not own.
    if let Some(existing) = manager.get_by_host(&host)? {
        anyhow::bail!(
            "host '{}' is already used by tenant '{}' ({})",
            host,
            existing.name,
            existing.id
        );
    }
    let db_name = db_name.unwrap_or_else(|| slugify(&name));
    let database = core_database(&cli.database, db_name.clone())?;

    let password_hash = ferris_auth::password::hash_password_default(&admin_password)
        .map_err(|e| anyhow::anyhow!("failed to hash admin password: {e}"))?;

    let tenant = TenantConfig::new(
        name.clone(),
        host.clone(),
        database,
        cli.redis.url.clone(),
        PathBuf::from(&cli.files.local_path).join(slugify(&name)),
        admin_email.clone(),
        password_hash,
    );

    // Provision: files directory + database pool (creates a SQLite file and the
    // migration bookkeeping table; for servers the database must already exist)
    // and the real admin user. The login lives in the tenant's own `User` table,
    // which is what `UserStore` creates; the hash kept in the tenant file is
    // metadata, not the credential.
    std::fs::create_dir_all(&tenant.files_path)?;
    let rt = crate::runtime::block_on_runtime(None)?;
    let pool = rt.block_on(async { ferris_db::create_pool(&tenant.database).await })?;
    rt.block_on(async {
        let store = ferris_auth::UserStore::new(pool);
        store.init().await?;
        store
            .create(
                &admin_email,
                &name,
                None,
                ferris_auth::UserType::SystemUser,
                &admin_password,
                &["System Manager".to_string()],
            )
            .await?;
        Ok::<(), ferris_core::Error>(())
    })?;

    manager.create(tenant.clone())?;
    info!("Tenant '{}' created", tenant.name);
    info!("  id:       {}", tenant.id);
    info!("  host:     {}", tenant.host);
    info!("  database: {}", db_name);
    info!("  admin:    {}", admin_email);
    info!("  files:    {}", tenant.files_path.display());
    Ok(())
}

fn list_tenants(config: &CliConfig) -> Result<()> {
    let manager = manager(config);
    // `load()` rather than `list()` so a tenant file that will not parse is
    // reported instead of the tenant silently vanishing from the listing.
    let load = manager.load()?;
    for skipped in &load.skipped {
        warn!("Skipping {}: {}", skipped.path.display(), skipped.reason);
    }
    let tenants = load.tenants;
    if tenants.is_empty() {
        info!("No tenants registered ({}).", manager.root().display());
        return Ok(());
    }
    for tenant in tenants {
        println!(
            "{:<24} {:<32} {:<12} {}",
            tenant.name,
            tenant.host,
            if tenant.enabled {
                "enabled"
            } else {
                "disabled"
            },
            tenant.database.name
        );
    }
    Ok(())
}

fn get_tenant(config: &CliConfig, id: &str) -> Result<()> {
    let tenant = manager(config).find(id)?;
    println!("id:         {}", tenant.id);
    println!("name:       {}", tenant.name);
    println!("host:       {}", tenant.host);
    println!("enabled:    {}", tenant.enabled);
    println!(
        "database:   {:?} {}",
        tenant.database.r#type, tenant.database.name
    );
    println!(
        "redis:      {}",
        super::config::redact_url(&tenant.redis_url)
    );
    println!("files:      {}", tenant.files_path.display());
    println!("admin:      {}", tenant.admin_email);
    println!("apps:       {}", tenant.enabled_apps.join(", "));
    println!("created:    {}", tenant.created_at);
    Ok(())
}

fn delete_tenant(config: &CliConfig, id: &str, force: bool) -> Result<()> {
    if !force {
        use dialoguer::Confirm;
        let confirm = Confirm::new()
            .with_prompt(format!(
                "Delete tenant '{id}'? This removes only its configuration. \
                 The database and files are kept."
            ))
            .default(false)
            .interact()?;
        if !confirm {
            info!("Deletion cancelled");
            return Ok(());
        }
    }
    let manager = manager(config);
    let tenant = manager.find(id)?;
    manager.delete(&tenant.id)?;
    info!("Tenant '{}' configuration deleted", tenant.name);
    // Data removal is deliberately out of scope: dropping a database or a files
    // root cannot be undone, and this CLI has no verified backup. An operator
    // who wants the data gone does it with the database's own tooling.
    warn!(
        "Database '{}' and files '{}' were not deleted; remove them with your \
         database/backup tooling.",
        tenant.database.name,
        tenant.files_path.display()
    );
    Ok(())
}

fn migrate_tenant(config: &CliConfig, id: &str, target: Option<u32>) -> Result<()> {
    let tenant = manager(config).find(id)?;
    let rt = crate::runtime::block_on_runtime(None)?;
    rt.block_on(async {
        let pool = ferris_db::create_pool(&tenant.database).await?;

        let mut runner = ferris_schema::MigrationRunner::for_db(&pool);
        runner.add_schema_migrations()?;

        let results = runner.run(&pool, target).await?;
        if results.is_empty() {
            info!("Tenant '{}': no pending migrations.", tenant.name);
        }
        for result in &results {
            if result.success {
                info!(
                    "Tenant '{}': ✓ {} ({}) in {}ms",
                    tenant.name, result.version, result.name, result.execution_time_ms
                );
            } else {
                error!(
                    "Tenant '{}': ✗ {} ({}) failed: {}",
                    tenant.name,
                    result.version,
                    result.name,
                    result.error.as_deref().unwrap_or("unknown error")
                );
            }
        }

        // A failed migration is a failed command. This logged a warning and
        // returned `Ok`, so `ferris tenant migrate` exited 0 with a broken
        // schema, and anything scripted on that exit code carried on.
        let failed = results.iter().filter(|r| !r.success).count();
        if failed > 0 {
            return Err(anyhow::anyhow!(
                "tenant '{}': {failed} migration(s) failed",
                tenant.name
            ));
        }

        Ok(())
    })
}

fn set_enabled(config: &CliConfig, id: &str, enabled: bool) -> Result<()> {
    let manager = manager(config);
    let tenant = manager.find(id)?;
    let tenant = manager.set_enabled(&tenant.id, enabled)?;
    if enabled {
        info!("Tenant '{}' enabled", tenant.name);
    } else {
        warn!("Tenant '{}' disabled", tenant.name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_admin_password_comes_from_wherever_it_can() {
        // The flag still wins, so existing scripts keep working.
        let (password, source) =
            resolve_admin_password_with(Some("from-flag".into()), Some("from-env".into()), || {
                panic!("must not prompt when a password was given")
            })
            .unwrap();
        assert_eq!(
            (password.as_str(), source),
            ("from-flag", PasswordSource::Flag)
        );

        // Then the environment, which does not show up in `ps`.
        let (password, source) = resolve_admin_password_with(None, Some("from-env".into()), || {
            panic!("must not prompt when the environment has it")
        })
        .unwrap();
        assert_eq!(
            (password.as_str(), source),
            ("from-env", PasswordSource::Environment)
        );

        // And then the terminal.
        let (password, source) =
            resolve_admin_password_with(None, None, || Ok("typed".into())).unwrap();
        assert_eq!(
            (password.as_str(), source),
            ("typed", PasswordSource::Prompt)
        );
    }

    #[test]
    fn an_empty_password_is_asked_for_rather_than_accepted() {
        // `--admin-password ""` used to create a tenant whose admin password was
        // the empty string.
        let (password, source) =
            resolve_admin_password_with(Some(String::new()), Some(String::new()), || {
                Ok("typed".into())
            })
            .unwrap();
        assert_eq!(
            (password.as_str(), source),
            ("typed", PasswordSource::Prompt)
        );

        let err = resolve_admin_password_with(None, None, || Ok(String::new()))
            .expect_err("an empty answer is not a password");
        assert!(err.to_string().contains("cannot be empty"), "{err}");
    }

    #[test]
    fn the_tenant_store_belongs_to_the_project() {
        assert_eq!(
            tenants_root(&PathBuf::from("/srv/app"), None),
            PathBuf::from("/srv/app/tenants")
        );
        assert_eq!(
            tenants_root(&PathBuf::from("/srv/app"), Some("/etc/ferris/tenants")),
            PathBuf::from("/etc/ferris/tenants"),
            "an explicit store is still honoured"
        );
    }

    #[test]
    fn two_projects_do_not_share_a_tenant_store() {
        assert_ne!(
            tenants_root(&PathBuf::from("/srv/a"), None),
            tenants_root(&PathBuf::from("/srv/b"), None)
        );
    }

    #[test]
    fn an_empty_override_is_not_a_store() {
        let project = PathBuf::from("/srv/app");
        let empty: Option<String> = Some(String::new());
        let override_dir = empty.filter(|dir| !dir.is_empty());
        assert!(override_dir.is_none());
        assert_eq!(
            tenants_root(&project, override_dir.as_deref()),
            PathBuf::from("/srv/app/tenants")
        );
    }

    /// SEC-15/CORE-02: `--tenant` must move every process-wide data location
    /// before boot. This is the whole merge, without a database connection.
    #[test]
    fn a_tenant_rewrites_the_database_files_redis_and_apps() {
        let mut cli = CliConfig::default();
        let tenant = TenantConfig::new(
            "Acme".into(),
            "acme.example.com".into(),
            ferris_core::DatabaseConfig {
                r#type: ferris_core::DatabaseType::SQLite,
                host: "tenant-db".into(),
                port: 5433,
                name: "acme_dev".into(),
                user: "acme".into(),
                password: "secret".into(),
                max_connections: 7,
                ssl_mode: ferris_core::SslMode::VerifyFull,
                ..ferris_core::DatabaseConfig::default()
            },
            "redis://tenant-redis:6379".into(),
            PathBuf::from("/srv/acme/files"),
            "admin@acme.test".into(),
            "hash".into(),
        );

        apply_tenant(&mut cli, &tenant);

        assert_eq!(cli.database.r#type, "sqlite");
        assert_eq!(cli.database.host, "tenant-db");
        assert_eq!(cli.database.port, 5433);
        assert_eq!(cli.database.name, "acme_dev");
        assert_eq!(cli.database.user, "acme");
        assert_eq!(cli.database.max_connections, 7);
        assert_eq!(cli.database.ssl_mode, "verify-full");
        assert_eq!(cli.redis.url, "redis://tenant-redis:6379");
        assert_eq!(cli.files.local_path, "/srv/acme/files");
        assert!(cli.apps.enabled.is_empty());
    }

    /// The app list travels with the tenant, so a process serving it publishes
    /// exactly the tenant's components and no other install's.
    #[test]
    fn a_tenant_app_list_replaces_the_project_one() {
        let mut cli = CliConfig::default();
        cli.apps.enabled = vec!["project-app".into()];
        let mut tenant = TenantConfig::new(
            "Acme".into(),
            "acme.example.com".into(),
            ferris_core::DatabaseConfig::default(),
            "redis://localhost".into(),
            PathBuf::from("/srv/acme"),
            "admin@acme.test".into(),
            "hash".into(),
        );
        tenant.enabled_apps = vec!["facturacion".into()];

        apply_tenant(&mut cli, &tenant);
        assert_eq!(cli.apps.enabled, vec!["facturacion".to_string()]);
    }
}
