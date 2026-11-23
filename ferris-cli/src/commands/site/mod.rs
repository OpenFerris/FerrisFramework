//! `ferris site`: the multi-site layout under `sites/<site>/`, and the boot
//! path that makes `ferris serve/run/dev --site <name>` serve one of them.
//!
//! A site is Frappe's unit of deployment: its own database, files root, Redis
//! and plugin **instances**. This module is the operator's side of
//! [`ferris_core::site`] plus the one merge the server needs before it builds a
//! pool. The isolation model is the tenant one: **one process serves one site**,
//! requests are not routed by `Host`, and `--site` therefore rewrites the
//! process-wide data locations instead of installing a per-request switch.
//!
//! `site new` provisions a real operator (`UserStore`, `System Manager`) in the
//! site's database, like `tenant create`; `site delete` removes the
//! configuration only, because dropping a database or a files root cannot be
//! undone and this CLI has no verified backup.
//!
//! Here rather than in `serve.rs` because three entry points use the merge
//! (`serve`, `run`, `dev`), and the `421` Host allow-list belongs to the same
//! decision: a process is authoritative for its site's `host_name`.

use std::path::Path;

use anyhow::Result;
use ferris_core::site::{SiteConfig, SiteManager, is_valid_site_name};
use ferris_core::{DatabaseConfig as CoreDatabaseConfig, tenant};
use tracing::{info, warn};

mod args;
mod docperm;
pub use args::{SiteArgs, SiteCommand};

use crate::config::CliConfig;

/// Dispatch `ferris site`.
///
/// # Errors
///
/// Whatever the subcommand fails on: an invalid or duplicate name/host, a
/// database that cannot be opened, an app that is not in the catalogue, or an
/// I/O failure under `sites/`.
pub fn run(args: SiteArgs, config: &CliConfig) -> Result<()> {
    let manager = manager(config);
    match args.command {
        SiteCommand::New {
            name,
            host,
            db_name,
            db_type,
            admin_email,
            admin_password,
        } => create_site(
            &manager,
            config,
            NewSite {
                name,
                host,
                db_name,
                db_type,
                admin_email,
                admin_password,
            },
        ),
        SiteCommand::List => list_sites(&manager),
        SiteCommand::Info { site } => site_info(&manager, &site),
        SiteCommand::Delete { site, force } => delete_site(&manager, &site, force),
        SiteCommand::Enable { site } => set_enabled(&manager, &site, true),
        SiteCommand::Disable { site } => set_enabled(&manager, &site, false),
        SiteCommand::Migrate { site, target } => migrate_site(&manager, &site, target),
        SiteCommand::InstallApp { app, site, force } => install_app(&manager, &site, &app, force),
        SiteCommand::UninstallApp { app, site, purge } => {
            uninstall_app(&manager, &site, &app, purge)
        }
    }
}

/// The sites registry of this bench.
///
/// `FERRIS_SITES_DIR`/`FERRIS_APPS_DIR` win when set; otherwise the roots are
/// `<bench>/sites` and `<bench>/apps`. Resolving them from the bench root (not
/// the current directory) is what lets `ferris app install` work from inside
/// `apps/<app>/` without creating a second, empty catalogue there.
pub(in crate::commands) fn manager(config: &CliConfig) -> SiteManager {
    let root = &config.source.bench_root;
    let sites = std::env::var_os("FERRIS_SITES_DIR")
        .map_or_else(|| root.join("sites"), std::path::PathBuf::from);
    let apps = std::env::var_os("FERRIS_APPS_DIR")
        .map_or_else(|| root.join("apps"), std::path::PathBuf::from);
    SiteManager::new(sites, apps)
}

/// The arguments of `site new`, grouped so the provisioning function keeps a
/// readable signature.
struct NewSite {
    name: String,
    host: Option<String>,
    db_name: Option<String>,
    db_type: Option<String>,
    admin_email: String,
    admin_password: String,
}

/// Create the layout **and** the site's operator.
///
/// Order matters: the name and the `host_name` are validated first, then the
/// admin is provisioned, then the configuration is written. A duplicate host
/// discovered after the user was created would leave an account in a database
/// this site does not own (the `tenant create` bug D5 fixed). Provisioning is
/// idempotent — an existing login is reused instead of duplicated — so a retry
/// after a failure to write the config does not create a second admin.
fn create_site(manager: &SiteManager, cli: &CliConfig, new: NewSite) -> Result<()> {
    // `site_dir` validates the name before any database is opened: the name is a
    // path component under `sites/`.
    manager.site_dir(&new.name)?;
    if manager.exists(&new.name) {
        anyhow::bail!(
            "site `{}` already exists in {}",
            new.name,
            manager.sites_root().display()
        );
    }

    let mut site = SiteConfig::new(new.name.clone());
    if let Some(host) = new.host {
        site.host_name = host;
    }
    let db_name = new.db_name.unwrap_or_else(|| site.database.name.clone());
    site.database = database_for(cli, db_name, new.db_type.as_deref())?;
    site.redis_url.clone_from(&cli.redis.url);
    // A new site starts with the bench's language policy; the operator can
    // still edit `site_config.json` afterwards.
    site.default_locale.clone_from(&cli.i18n.default_locale);
    site.supported_locales.clone_from(&cli.i18n.normalized());

    ensure_host_free(manager, &site)?;
    provision_operator(manager, &site, &new.admin_email, &new.admin_password)?;

    let site = manager.create(site)?;
    info!("Site '{}' created", site.name);
    info!("  host:     {}", site.host_name);
    info!("  database: {}", site.database.name);
    info!("  admin:    {}", new.admin_email);
    info!("  files:    {}", site.files_path.display());
    info!(
        "  serve it with `ferris serve --site {}`",
        site.name.as_str()
    );
    Ok(())
}

/// The site's database as the project config describes it, with `--db-name` and
/// `--db-type` as the per-site overrides.
///
/// Everything else (host, port, credentials, `ssl_mode`) comes from
/// `[database]`: a site is another database on the same server, not another
/// server. `--db-type` is validated by the shared mapping, never guessed.
fn database_for(
    cli: &CliConfig,
    db_name: String,
    db_type: Option<&str>,
) -> Result<CoreDatabaseConfig> {
    let mut source = cli.database.clone();
    if let Some(kind) = db_type {
        source.r#type = kind.to_string();
    }
    super::db_map::core_database(&source, db_name)
}

/// A `host_name` another site already serves must be refused **before** a
/// database is touched, so the error names the conflict instead of leaving a
/// provisioned admin behind. [`SiteManager::create`] enforces the same rule
/// again; this is the pre-flight, not the policy.
fn ensure_host_free(manager: &SiteManager, site: &SiteConfig) -> Result<()> {
    let Some(host) = tenant::normalize_host(&site.host_name) else {
        return Ok(());
    };
    for other in manager.list()? {
        if other.name == site.name {
            continue;
        }
        if tenant::normalize_host(&other.host_name).as_deref() == Some(host.as_str()) {
            anyhow::bail!(
                "host `{host}` is already served by site `{}`; pick another with `--host`",
                other.name
            );
        }
    }
    Ok(())
}

/// Create the real admin user in the site's database.
///
/// Same shape as `tenant create`: `UserStore::init` creates the tables,
/// `create` writes the login the Desk authenticates against. Pointing a site at
/// a database that already exists is supported, so the login is looked up first
/// and an existing one is reported instead of duplicated (a second row for the
/// same email is not a second account, it is a broken install).
/// The site's database with a relative SQLite file anchored at the bench root.
///
/// `site_config.json` keeps the name the operator wrote (a relative file moves
/// with the bench). Opening it from an app directory must still hit the file
/// `site new` created, so the path is resolved here, once, for every command
/// that opens a site pool.
pub(super) fn site_database(
    manager: &SiteManager,
    site: &SiteConfig,
) -> ferris_core::DatabaseConfig {
    let mut database = site.database.clone();
    if database.r#type == ferris_core::DatabaseType::SQLite
        && !database.name.is_empty()
        && Path::new(&database.name).is_relative()
    {
        let bench_root = manager
            .sites_root()
            .parent()
            .unwrap_or_else(|| Path::new("."));
        database.name = bench_root.join(&database.name).display().to_string();
    }
    database
}

fn provision_operator(
    manager: &SiteManager,
    site: &SiteConfig,
    admin_email: &str,
    admin_password: &str,
) -> Result<()> {
    let database = site_database(manager, site);
    let rt = crate::runtime::block_on_runtime(None)?;
    rt.block_on(async {
        let pool = match ferris_db::create_pool(&database).await {
            Ok(pool) => pool,
            Err(error) => anyhow::bail!(
                "cannot open the database `{}` of site `{}` (create it first): {error}",
                site.database.name,
                site.name
            ),
        };
        let store = ferris_auth::UserStore::new(pool);
        store.init().await?;
        if let Some(existing) = store.find_by_login(admin_email).await? {
            info!(
                "user `{}` already exists in database `{}`; not creating another admin",
                existing.email, site.database.name
            );
            return Ok(());
        }
        store
            .create(
                admin_email,
                &site.name,
                None,
                ferris_auth::UserType::SystemUser,
                admin_password,
                &["System Manager".to_string()],
            )
            .await?;
        info!(
            "created admin `{admin_email}` in database `{}`",
            site.database.name
        );
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}

fn list_sites(manager: &SiteManager) -> Result<()> {
    let sites = manager.list()?;
    if sites.is_empty() {
        info!(
            "No sites registered ({}). Run `ferris site new <name>`.",
            manager.sites_root().display()
        );
        return Ok(());
    }
    println!(
        "{:<24} {:<28} {:<9} {:<20} APPS",
        "SITE", "HOST", "ENABLED", "DATABASE"
    );
    for site in sites {
        println!(
            "{:<24} {:<28} {:<9} {:<20} {}",
            site.name,
            site.host_name,
            if site.enabled { "yes" } else { "no" },
            site.database.name,
            site.apps.join(", ")
        );
    }
    Ok(())
}

fn site_info(manager: &SiteManager, name: &str) -> Result<()> {
    let site = manager.get(name)?;
    let plugins = manager.plugins_root(&site.name)?;
    println!("name:       {}", site.name);
    println!("host:       {}", site.host_name);
    println!("enabled:    {}", site.enabled);
    match site.database.r#type {
        ferris_core::DatabaseType::SQLite => {
            println!("database:   SQLite {}", site.database.name);
        }
        kind => {
            println!(
                "database:   {kind:?} {} on {}:{}",
                site.database.name, site.database.host, site.database.port
            );
        }
    }
    println!("redis:      {}", crate::config::redact_url(&site.redis_url));
    println!("files:      {}", site.files_path.display());
    println!("plugins:    {}", plugins.display());
    println!("apps:       {}", site.apps.join(", "));
    let versions: Vec<String> = site
        .apps
        .iter()
        .map(|app| {
            site.app_versions
                .get(app)
                .map_or_else(|| format!("{app}=?"), |version| format!("{app}={version}"))
        })
        .collect();
    println!("versions:   {}", versions.join(", "));
    let overrides: Vec<&str> = site.plugins.keys().map(String::as_str).collect();
    println!("overrides:  {}", overrides.join(", "));
    println!("created:    {}", site.created_at);
    println!("updated:    {}", site.updated_at);
    Ok(())
}

fn delete_site(manager: &SiteManager, name: &str, force: bool) -> Result<()> {
    let site = manager.get(name)?;
    if !force {
        use dialoguer::Confirm;
        let confirm = Confirm::new()
            .with_prompt(format!(
                "Delete site '{}'? This removes only its configuration. \
                 Its database and files are kept.",
                site.name
            ))
            .default(false)
            .interact()?;
        if !confirm {
            info!("Deletion cancelled");
            return Ok(());
        }
    }
    manager.delete(&site.name)?;
    info!("Site '{}' configuration deleted", site.name);
    // Data removal is deliberately out of scope (see the module docs): dropping
    // a database or a files tree is irreversible.
    warn!(
        "Database '{}' and files '{}' were not deleted; remove them with your \
         database/backup tooling.",
        site.database.name,
        site.files_path.display()
    );
    Ok(())
}

fn set_enabled(manager: &SiteManager, name: &str, enabled: bool) -> Result<()> {
    let site = manager.set_enabled(name, enabled)?;
    if enabled {
        info!("Site '{}' enabled", site.name);
    } else {
        warn!("Site '{}' disabled; it keeps its files and data", site.name);
    }
    Ok(())
}

/// Apply the site's migrations to **its** database.
///
/// The app worlds' customizations are re-applied afterwards, the same way
/// `ferris migrate` does it for the project: a site's world is what makes an
/// exported customization effective on a fresh database.
fn migrate_site(manager: &SiteManager, name: &str, target: Option<u32>) -> Result<()> {
    let site = manager.get(name)?;
    let materialized = manager.materialize_instance_components(name)?;
    if materialized > 0 {
        info!(
            "Site '{}': materialized {} legacy symlinked component(s) into copies",
            site.name, materialized
        );
    }
    let plugins = manager.plugins_root(&site.name)?;
    let database = site_database(manager, &site);
    let rt = crate::runtime::block_on_runtime(None)?;
    rt.block_on(async {
        let pool = ferris_db::create_pool(&database).await?;

        let mut runner = ferris_schema::MigrationRunner::for_db(&pool);
        runner.add_schema_migrations()?;

        let results = runner.run(&pool, target).await?;
        if results.is_empty() {
            info!("Site '{}': no pending migrations.", site.name);
        }
        for result in &results {
            if result.success {
                info!(
                    "Site '{}': ✓ {} ({}) in {}ms",
                    site.name, result.version, result.name, result.execution_time_ms
                );
            } else {
                warn!(
                    "Site '{}': ✗ {} ({}) failed: {}",
                    site.name,
                    result.version,
                    result.name,
                    result.error.as_deref().unwrap_or("unknown error")
                );
            }
        }

        // A failed migration is a failed command: an exit code of 0 with a
        // broken schema is what scripts key off.
        let failed = results.iter().filter(|result| !result.success).count();
        if failed > 0 {
            return Err(anyhow::anyhow!(
                "site '{}': {failed} migration(s) failed",
                site.name
            ));
        }

        let applied = super::plugin_world::apply_all(&pool, &plugins, &site.apps).await?;
        if applied > 0 {
            info!(
                "Site '{}': applied {applied} plugin customization(s) from {}.",
                site.name,
                plugins.display()
            );
        }
        Ok(())
    })
}

fn install_app(manager: &SiteManager, site: &str, app: &str, force: bool) -> Result<()> {
    let catalogue = manager.app_manifest(app)?;
    #[cfg(feature = "wasm")]
    super::app::validate_component(&catalogue.wasm, &catalogue.name)?;
    // The catalogue was validated when it was installed; this catches a file
    // edited in place (or copied by hand) before it reaches the site's boot.
    let package = manager.global_app_dir(app)?;
    let tags = super::app::locales::validate(&package, None)?;
    if !tags.is_empty() {
        info!("App '{app}' translates itself into: {}", tags.join(", "));
    }
    let manifest = manager.install_app(site, app, force)?;
    let applied = docperm::apply(manager, site, app)?;
    if applied > 0 {
        info!("Applied {applied} permission row(s) from the app package");
    }
    let instance = manager.instance_world(site, app)?;
    info!(
        "Installed {} {} in site '{}' at {}",
        manifest.name,
        manifest.version,
        site,
        instance.display()
    );
    Ok(())
}

fn uninstall_app(manager: &SiteManager, site: &str, app: &str, purge: bool) -> Result<()> {
    manager.uninstall_app(site, app, purge)?;
    if purge {
        warn!(
            "Site '{site}': purged the instance of `{app}` (its data, files and secrets). \
             The global catalogue is untouched."
        );
    } else {
        info!(
            "Site '{site}': removed `{app}` from apps.txt; its instance data is kept \
             (use --purge to delete it)."
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Boot path: `--site` on serve / run / dev
// ---------------------------------------------------------------------------

/// Point this process at a site before any settings are built.
///
/// Mirrors `tenant::apply_tenant_by_id`, with the site's plugin **instances** as
/// the published app directory: `apps.wasm_dir = sites/<site>/plugins`, which
/// `plugin_boot::bootstrap` already understands (`<app>/<app>.wasm` inside the
/// directory is the instance's symlink to the global component).
///
/// A disabled site is refused rather than served half-way. The `Host`
/// allow-list is installed here for the same reason the site exists: this
/// process is authoritative for `host_name` and must answer `421` to anything
/// else.
pub(in crate::commands) fn apply_site_by_name(config: &mut CliConfig, name: &str) -> Result<()> {
    let manager = manager(config);
    if !manager.exists(name) {
        anyhow::bail!(
            "site `{name}` does not exist in {}; run `ferris site list`",
            manager.sites_root().display()
        );
    }
    let mut site = manager.get(name)?;
    usable(&site)?;
    // Upgrade a pre-copy installation in place: the site keeps serving its
    // pinned component instead of whatever the catalogue contains next.
    let materialized = manager.materialize_instance_components(name)?;
    if materialized > 0 {
        info!(
            "Site '{}': materialized {} legacy symlinked component(s) into copies",
            site.name, materialized
        );
    }
    // Anchor a relative SQLite file before it is copied into the CLI config,
    // even when `FERRIS_SITES_DIR` points somewhere other than `<bench>/sites`.
    site.database = site_database(&manager, &site);
    let plugins = manager.plugins_root(&site.name)?;
    // A `site_config.json` copied from Frappe carries no `files_path`. Empty
    // means "this site's private root", never "the process working directory".
    if site.files_path.as_os_str().is_empty() {
        site.files_path = manager.site_dir(&site.name)?.join("private/files");
    }
    info!(
        "Serving site '{}' (host '{}') from database '{}'",
        site.name, site.host_name, site.database.name
    );
    let hosts = allowed_hosts_for(config, &site)?;
    apply_site(config, &site, &plugins);
    install_allowed_hosts(&hosts)
}

/// A site this process may serve.
fn usable(site: &SiteConfig) -> Result<()> {
    if !site.enabled {
        anyhow::bail!(
            "site `{}` is disabled; run `ferris site enable {}` first",
            site.name,
            site.name
        );
    }
    if !is_valid_site_name(&site.name) {
        anyhow::bail!("site `{}` has an unusable name", site.name);
    }
    Ok(())
}

/// The merge itself, pure so it can be tested without a filesystem.
///
/// Everything the process reads at boot comes from the site: its database, its
/// files root, its Redis and its app list. `plugins_root` is the instance scope,
/// because that is the directory a guest mounts at `/world`.
pub(in crate::commands) fn apply_site(
    config: &mut CliConfig,
    site: &SiteConfig,
    plugins_root: &Path,
) {
    apply_database(config, &site.database);
    // `site new` created the SQLite file at the bench root. A command run from
    // an app directory must open that same file, not a second one next to the
    // app, so a relative name is anchored before any pool exists.
    if config.database.r#type == "sqlite"
        && !config.database.name.is_empty()
        && Path::new(&config.database.name).is_relative()
    {
        config.database.name = config
            .source
            .bench_root
            .join(&config.database.name)
            .display()
            .to_string();
    }
    config.redis.url.clone_from(&site.redis_url);
    config.files.local_path = site.files_path.display().to_string();
    config.apps.enabled.clone_from(&site.apps);
    config.apps.wasm_dir = plugins_root.to_path_buf();
    // The site is the deployment unit: its language policy wins over the
    // bench default exactly like its database and files root.
    config.i18n.default_locale.clone_from(&site.default_locale);
    config.i18n.supported.clone_from(&site.supported_locales);
}

/// Map the core database config back onto the CLI's string-typed one.
///
/// The inverse of [`super::db_map::core_database`] (which is what `tenant`
/// uses). It lives here because the site boot path must not depend on the tenant
/// registry; `tenant::apply_tenant` maps the same fields and should delegate
/// here the next time that file is touched.
fn apply_database(config: &mut CliConfig, database: &CoreDatabaseConfig) {
    config.database.r#type = match database.r#type {
        ferris_core::DatabaseType::Postgres => "postgres",
        ferris_core::DatabaseType::MySQL => "mysql",
        ferris_core::DatabaseType::SQLite => "sqlite",
    }
    .to_string();
    config.database.host.clone_from(&database.host);
    config.database.port = database.port;
    config.database.name.clone_from(&database.name);
    config.database.user.clone_from(&database.user);
    config.database.password.clone_from(&database.password);
    config.database.max_connections = database.max_connections;
    config.database.ssl_mode = match database.ssl_mode {
        ferris_core::SslMode::Disable => "disable",
        ferris_core::SslMode::Allow => "allow",
        ferris_core::SslMode::Prefer => "prefer",
        ferris_core::SslMode::Require => "require",
        ferris_core::SslMode::VerifyCa => "verify-ca",
        ferris_core::SslMode::VerifyFull => "verify-full",
    }
    .to_string();
}

/// The names this process answers for: the file's `[server] allowed_hosts` plus
/// the site's `host_name`.
///
/// The file's list is kept so an operator can add a proxy name, but the site's
/// host is always present: that is the `421 Misdirected Request` contract of D5.
fn allowed_hosts_for(config: &CliConfig, site: &SiteConfig) -> Result<Vec<String>> {
    let mut hosts = allowed_hosts_from_file(&config.source.config_path)?;
    let wanted = site.host_name.trim();
    if !wanted.is_empty()
        && !hosts
            .iter()
            .any(|host| tenant::normalize_host(host) == tenant::normalize_host(wanted))
    {
        hosts.push(wanted.to_string());
    }
    Ok(hosts)
}

/// Install the process-wide `Host` allow-list, tolerating an identical re-install.
///
/// The list is a `OnceLock`: a second, *different* installation is a boot error
/// rather than a silent replacement. `ferris dev --hot-reload front` re-enters
/// the dev path for the same site, which installs the same list twice; that is
/// the only case treated as a no-op.
fn install_allowed_hosts(hosts: &[String]) -> Result<()> {
    if hosts.is_empty() {
        return Ok(());
    }
    let installed = tenant::allowed_hosts();
    if !installed.is_empty() {
        if installed == hosts {
            return Ok(());
        }
        anyhow::bail!(
            "the Host allow-list was already installed with a different list; \
             one process serves one site"
        );
    }
    if !tenant::install_allowed_hosts(hosts.to_vec()) {
        anyhow::bail!("the Host allow-list was already installed for this process");
    }
    Ok(())
}

/// Read `[server] allowed_hosts` from the project file; an absent key is the
/// documented default (no restriction).
///
/// Read from the TOML rather than `CliConfig` because the key is not part of the
/// CLI's `ServerConfig`; the router adds `external_url` itself.
pub(in crate::commands) fn allowed_hosts_from_file(path: &Path) -> Result<Vec<String>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let value: toml::Value = toml::from_str(&text)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?;
    let Some(entries) = value
        .get("server")
        .and_then(|server| server.get("allowed_hosts"))
    else {
        return Ok(Vec::new());
    };
    let Some(entries) = entries.as_array() else {
        anyhow::bail!("server.allowed_hosts must be an array of host names");
    };
    entries
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("server.allowed_hosts entries must be strings"))
        })
        .collect()
}

#[cfg(test)]
mod tests;
