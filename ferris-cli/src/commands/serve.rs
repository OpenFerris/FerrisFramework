//! `ferris serve`: run the unified server on a single port with flag-selected roles.

use anyhow::Result;
use ferris_core::i18n::{I18n, Locale};
use ferris_core::{Config as CoreConfig, DatabaseConfig as CoreDatabaseConfig};
use ferris_server::{Server, ServerRole, ServerRoles, ServerSettings};

use super::ServeArgs;
use crate::config::{CliConfig, DatabaseConfig};
use crate::features;

pub fn run(args: ServeArgs, config: &CliConfig) -> Result<()> {
    if args.all_sites {
        return super::multi_site::run(args, config);
    }
    let roles = roles_from_flags(&args, config)?;

    // `--cors-origin` flags extend (and imply) the config-driven allow-list.
    let mut effective = config.clone();
    if !args.cors_origin.is_empty() {
        effective.server.enable_cors = true;
        effective
            .server
            .cors_origins
            .extend(args.cors_origin.iter().cloned());
    }
    // A site/tenant rewrites the process-wide data locations *before* any
    // settings are built, which is what makes "one instance per site" real: the
    // pool, the files root and the published apps never see another site's
    // values. `--site` also installs the site's `Host` allow-list, so the
    // file-driven installation below must not run on that path.
    if let Some(name) = args.site.as_deref() {
        super::site::apply_site_by_name(&mut effective, name)?;
    } else {
        if let Some(id) = args.tenant.as_deref() {
            tracing::warn!(
                "--tenant is the pre-sites model; `ferris serve --site <name>` uses the \
                 sites/ layout and provisions the same per-process isolation"
            );
            super::tenant::apply_tenant_by_id(&mut effective, id)?;
        }
        apply_allowed_hosts(&effective)?;
    }
    let settings = settings_for(args.host, args.port, roles, &effective)?;
    start(settings, &effective)
}

/// Install `[server] allowed_hosts` for this process.
///
/// Read from the same TOML the loader read, because the key is not part of
/// `CliConfig::ServerConfig`; the router adds `external_url` itself.
pub(in crate::commands) fn apply_allowed_hosts(config: &CliConfig) -> Result<()> {
    let hosts = super::site::allowed_hosts_from_file(&config.source.config_path)?;
    if !hosts.is_empty() && !ferris_core::tenant::install_allowed_hosts(hosts) {
        anyhow::bail!("the Host allow-list was already installed for this process");
    }
    Ok(())
}

fn roles_from_flags(args: &ServeArgs, config: &CliConfig) -> Result<ServerRoles> {
    if args.all {
        return Ok(ServerRoles::all());
    }
    let mut selected = Vec::new();
    for (flag, role) in [
        (args.api, ServerRole::Api),
        (args.web, ServerRole::Web),
        (args.realtime, ServerRole::Realtime),
        (args.worker, ServerRole::Worker),
        (args.scheduler, ServerRole::Scheduler),
    ] {
        if flag {
            selected.push(role.as_str().to_string());
        }
    }
    // No flag falls back to `[server] roles`, the same source `ferris run`
    // reads: `ferris serve --site <name>` is a documented command and must not
    // fail because it omitted the roles the config already names.
    if selected.is_empty() {
        if config.server.roles.is_empty() {
            anyhow::bail!(
                "no roles selected: pass --all, one of --api/--web/--realtime/--worker/--scheduler, \
                 or list them in [server] roles"
            );
        }
        return ServerRoles::parse(&config.server.roles);
    }
    ServerRoles::parse(&selected)
}

/// Build `ServerSettings` from CLI flags plus the project config.
pub(in crate::commands) fn settings_for(
    host: String,
    port: u16,
    roles: ServerRoles,
    config: &CliConfig,
) -> Result<ServerSettings> {
    settings_for_with_features(host, port, roles, config, &features::compiled())
}

/// [`settings_for`] with an explicit feature set.
///
/// The set is a parameter so a test can exercise a minimal build
/// (`--no-default-features --features api`) while running inside the
/// full-featured test binary, and so the boot pre-flight and `ferris doctor`
/// answer from the same value.
pub(in crate::commands) fn settings_for_with_features(
    host: String,
    port: u16,
    roles: ServerRoles,
    config: &CliConfig,
    compiled: &ferris_core::host_features::HostFeatures,
) -> Result<ServerSettings> {
    // The feature matrix is checked first: a config that asks for any
    // other subsystem) this binary was not built with must fail before a
    // database pool is opened or a socket is bound, and the message must name
    // the Cargo features to rebuild with. Every entry point that starts a
    // server goes through here, so `serve`, `run` and `dev` cannot disagree.
    features::validate(config, compiled)?;
    // A zero-byte limit is not "no cap": it makes every body-bearing request
    // exceed the ceiling before a handler can read it. Reject it at boot with
    // the key the operator wrote, instead of serving a process that answers
    // 413 for everything.
    if config.server.body_limit_mb == 0 {
        anyhow::bail!("server.body_limit_mb must be greater than zero");
    }
    let core_config = build_core_config(config)?;
    let mut settings = ServerSettings::new(
        host,
        port,
        roles,
        database_config(&config.database)?,
        core_config,
    );
    settings.request_timeout_secs = config.server.request_timeout_secs;
    // The router enforces this field, not `settings.config.server.body_limit_mb`.
    settings.body_limit_mb = config.server.body_limit_mb;
    settings.enable_cors = config.server.enable_cors;
    settings.cors_origins = config.server.cors_origins.clone();
    settings.i18n = build_i18n(&config.i18n);
    Ok(settings)
}

/// Build the locale policy from `[i18n]`.
///
/// An unparseable tag is dropped (and the default falls back to English)
/// instead of refusing to boot: a locale is presentation, and a typo in
/// `supported` must not take the API down. `ferris doctor` prints the set, so
/// the typo is still visible.
fn build_i18n(config: &crate::config::I18nConfig) -> I18n {
    let default = Locale::parse(&config.default_locale).unwrap_or_default();
    let supported: Vec<Locale> = config
        .supported
        .iter()
        .filter_map(|tag| Locale::parse(tag))
        .collect();
    I18n::new(default, supported)
}

fn build_core_config(cli_config: &CliConfig) -> Result<CoreConfig> {
    let jwt_secret = cli_config.require_jwt_secret()?.to_string();

    Ok(CoreConfig {
        database: database_config(&cli_config.database)?,
        redis: ferris_core::RedisConfig {
            url: cli_config.redis.url.clone(),
            max_connections: cli_config.redis.max_connections,
            connection_timeout_secs: 5,
            command_timeout_secs: 30,
        },
        server: ferris_core::ServerConfig {
            host: cli_config.server.host.clone(),
            port: cli_config.server.port,
            workers: cli_config.server.workers,
            request_timeout_secs: cli_config.server.request_timeout_secs,
            body_limit_mb: cli_config.server.body_limit_mb,
            enable_cors: cli_config.server.enable_cors,
            cors_origins: cli_config.server.cors_origins.clone(),
            trusted_proxies: vec![],
            tls: cli_config.server.tls.clone(),
            pki: cli_config.server.pki.clone(),
            external_url: cli_config.server.external_url.clone(),
            roles: cli_config.server.roles.clone(),
        },
        auth: ferris_core::AuthConfig {
            jwt_secret: Some(jwt_secret),
            jwt_expiry_hours: cli_config.auth.jwt_expiry_hours,
            refresh_expiry_days: cli_config.auth.refresh_expiry_days,
            password_min_length: cli_config.auth.password_min_length,
            password_require_uppercase: true,
            password_require_lowercase: true,
            password_require_numbers: true,
            password_require_symbols: false,
            max_login_attempts: 5,
            lockout_duration_minutes: 15,
            mfa_enabled: cli_config.auth.mfa_enabled,
            mfa_issuer: "Ferris".into(),
            session_timeout_minutes: 60,
            // The CLI config does not expose this knob yet; the core default is
            // five concurrent sessions (see `Config::default` in `ferris-core`).
            max_sessions_per_user: 5,
            oauth: cli_config.auth.oauth.clone(),
        },
        files: cli_config.files.to_core(),
        email: cli_config.email.clone(),
        search: cli_config.search.clone(),
        realtime: cli_config.realtime.clone(),
        jobs: cli_config.jobs.clone(),
        scheduler: cli_config.scheduler.clone(),
        retention: Default::default(),
        features: cli_config.features.clone(),
        logging: Default::default(),
        telemetry: Default::default(),
        // The server-side copy of the app list. `serve::start` reads the same
        // values to publish the components, so the two cannot drift apart.
        apps: cli_config.apps.clone(),
        // Key material for `Secret` fields (H3). The server installs it before
        // serving; carrying it here is what makes `[secrets]`/`FERRIS_SECRET_KEY`
        // reach that point instead of being silently dropped.
        secrets: cli_config.secrets.clone(),
    })
}

/// Run a server; blocks until it shuts down.
///
/// The plugin store is built here, before `Server::run`, because `build_router`
/// is what mounts `/plugins` and registers one `/api/method/<name>` handler per
/// declared method: a store created afterwards would publish nothing until the
/// next restart. One runtime and one database pool are shared by the boot path
/// and the server, so no second set of worker threads or connections is opened.
pub(in crate::commands) fn start(settings: ServerSettings, config: &CliConfig) -> Result<()> {
    // Only the `wasm` boot path mutates `settings` (to attach the plugin store)
    // or reads `config` (the `[apps]` table); the minimal build must not warn
    // about either.
    #[cfg(feature = "wasm")]
    let mut settings = settings;
    #[cfg(not(feature = "wasm"))]
    let _ = config;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    #[cfg(feature = "wasm")]
    if needs_plugin_boot(&settings) {
        let pool = match settings.pool.clone() {
            Some(pool) => pool,
            None => {
                let pool = runtime.block_on(ferris_db::create_pool(&settings.database))?;
                settings.pool = Some(pool.clone());
                pool
            }
        };
        if let Some(store) =
            super::plugin_boot::bootstrap(&runtime, pool, &config.apps, &config.source.project_dir)?
        {
            settings.plugins = Some(store);
        }
    }

    runtime.block_on(Server::new(settings).run())
}

/// Whether this process still has to publish the apps from `[apps]`.
///
/// `ferris dev` loads the app it just built and sets `plugins` itself, so the
/// server boot must not load the directory on top of it.
#[cfg(feature = "wasm")]
const fn needs_plugin_boot(settings: &ServerSettings) -> bool {
    settings.plugins.is_none()
}

/// Connections the server keeps open for the life of the process. A dev command
/// does not want these: it opens a pool for one migration and drops it.
const SERVER_MIN_CONNECTIONS: u32 = 5;

fn database_config(cli: &DatabaseConfig) -> Result<CoreDatabaseConfig> {
    let mut config = super::db_map::core_database(cli, cli.name.clone())?;
    config.min_connections = SERVER_MIN_CONNECTIONS;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_core::DatabaseType;

    fn serve_args() -> ServeArgs {
        ServeArgs {
            host: "0.0.0.0".into(),
            port: 8000,
            api: false,
            web: false,
            realtime: false,
            worker: false,
            scheduler: false,
            all: false,
            cors_origin: Vec::new(),
            site: None,
            tenant: None,
            all_sites: false,
        }
    }

    #[test]
    fn no_flags_uses_the_configured_roles() {
        let mut config = CliConfig::default();
        config.server.roles = vec!["api".into(), "web".into()];
        let roles = roles_from_flags(&serve_args(), &config).unwrap();
        assert!(roles.has(ServerRole::Api));
        assert!(roles.has(ServerRole::Web));
        assert!(!roles.has(ServerRole::Realtime));
    }

    #[test]
    fn no_flags_and_no_configured_roles_is_an_error() {
        let mut config = CliConfig::default();
        config.server.roles.clear();
        assert!(roles_from_flags(&serve_args(), &config).is_err());
    }

    #[test]
    fn single_flags_selected() {
        let mut args = serve_args();
        args.api = true;
        args.web = true;
        let roles = roles_from_flags(&args, &CliConfig::default()).unwrap();
        assert!(roles.has(ServerRole::Api));
        assert!(roles.has(ServerRole::Web));
        assert!(!roles.has(ServerRole::Realtime));
    }

    #[test]
    fn all_flag_enables_every_role() {
        let mut args = serve_args();
        args.all = true;
        let roles = roles_from_flags(&args, &CliConfig::default()).unwrap();
        for role in ServerRole::ALL {
            assert!(roles.has(role));
        }
    }

    /// The section-to-core mapping is the whole point of the CLI config: before
    /// this, `serve` passed `Default::default()` for files/email/search/realtime
    /// and hardcoded the jobs/scheduler knobs, so the TOML never reached the
    /// server.
    #[test]
    fn maps_every_config_section_into_the_core_config() {
        use ferris_core::StorageBackend;

        let mut cli = CliConfig::default();
        cli.auth.jwt_secret = Some("a".repeat(32));
        cli.server.body_limit_mb = 64;
        cli.files.backend = StorageBackend::Memory;
        cli.files.local_path = "/srv/files".to_string();
        cli.files.max_file_size_mb = 123;
        cli.files.allowed_extensions = vec!["pdf".to_string()];
        cli.files.image_quality = 70;
        cli.email.queue_concurrency = 4;
        cli.email.retry_attempts = 9;
        cli.search.enabled = false;
        cli.realtime.port = 9100;
        cli.jobs.worker_concurrency = 6;
        cli.jobs.default_timeout_secs = 77;
        cli.jobs.max_retries = 4;
        cli.jobs.retry_backoff_base_secs = 15;
        cli.jobs.dead_letter_queue = "dlq".to_string();
        cli.jobs.queues = vec![ferris_core::QueueConfig {
            name: "reports".to_string(),
            priority: 3,
            concurrency: 2,
            timeout_secs: 90,
        }];
        cli.scheduler.timezone = "Europe/Madrid".to_string();
        cli.scheduler.history_retention_days = 7;
        cli.scheduler.tick_interval_secs = 15;
        cli.scheduler.max_concurrent_jobs = 9;
        cli.features.realtime = false;
        cli.features.email = false;
        cli.features.report = false;

        let core = build_core_config(&cli).unwrap();
        assert_eq!(core.server.body_limit_mb, 64);
        assert_eq!(core.files.backend, StorageBackend::Memory);
        assert_eq!(
            core.files.local_path,
            std::path::PathBuf::from("/srv/files")
        );
        assert_eq!(core.files.max_file_size_mb, 123);
        assert_eq!(core.files.allowed_extensions, vec!["pdf".to_string()]);
        assert_eq!(core.files.image_quality, 70);
        assert_eq!(core.email.queue_concurrency, 4);
        assert_eq!(core.email.retry_attempts, 9);
        assert!(!core.search.enabled);
        assert_eq!(core.realtime.port, 9100);
        assert_eq!(core.jobs.worker_concurrency, 6);
        assert_eq!(core.jobs.default_timeout_secs, 77);
        assert_eq!(core.jobs.max_retries, 4);
        assert_eq!(core.jobs.retry_backoff_base_secs, 15);
        assert_eq!(core.jobs.dead_letter_queue, "dlq");
        assert_eq!(core.jobs.queues.len(), 1);
        assert_eq!(core.jobs.queues[0].name, "reports");
        assert_eq!(core.scheduler.timezone, "Europe/Madrid");
        assert_eq!(core.scheduler.history_retention_days, 7);
        assert_eq!(core.scheduler.tick_interval_secs, 15);
        assert_eq!(core.scheduler.max_concurrent_jobs, 9);
        assert!(!core.features.realtime);
        assert!(!core.features.email);
        assert!(!core.features.report);
    }

    #[test]
    fn settings_for_applies_body_limit_and_keeps_the_requested_roles() {
        let mut cli = CliConfig::default();
        cli.auth.jwt_secret = Some("a".repeat(32));
        cli.server.body_limit_mb = 96;
        cli.server.request_timeout_secs = 12;

        let roles = ServerRoles::parse(&["api".to_string()]).unwrap();
        let settings = settings_for("127.0.0.1".into(), 0, roles, &cli).unwrap();

        // `ServerSettings.body_limit_mb` is the field the router reads; the core
        // copy must agree so a future consumer does not see a different limit.
        assert_eq!(settings.body_limit_mb, 96);
        assert_eq!(settings.config.server.body_limit_mb, 96);
        assert_eq!(settings.request_timeout_secs, 12);
        assert!(settings.roles.has(ServerRole::Api));
        assert!(!settings.roles.has(ServerRole::Web));
        assert!(settings.config.features.email);
    }

    #[test]
    fn a_zero_body_limit_is_rejected() {
        let mut cli = CliConfig::default();
        cli.auth.jwt_secret = Some("a".repeat(32));
        cli.server.body_limit_mb = 0;
        let roles = ServerRoles::parse(&["api".to_string()]).unwrap();
        let err = settings_for("127.0.0.1".into(), 0, roles, &cli)
            .expect_err("a zero-byte body limit must fail at boot");
        assert!(err.to_string().contains("body_limit_mb"), "{err}");
    }

    /// A minimal build that says so in `[features]` boots normally: the check
    /// must complain about what is *asked for*, never about what was compiled
    /// out on purpose.
    #[test]
    fn a_minimal_build_with_every_gate_off_starts() {
        let mut cli = CliConfig::default();
        cli.auth.jwt_secret = Some("a".repeat(32));
        quiet_gates(&mut cli);

        let roles = ServerRoles::parse(&["api".to_string()]).unwrap();
        let compiled = ferris_core::host_features::HostFeatures::new(["api"]);
        let settings = settings_for_with_features("127.0.0.1".into(), 0, roles, &cli, &compiled)
            .expect("an api-only build with nothing else requested is valid");
        assert!(settings.roles.has(ServerRole::Api));
        assert!(!settings.roles.has(ServerRole::Web));
    }

    /// `[features]` is authoritative at boot: a disabled feature removes its
    /// role even if the flags asked for it, and an enabled feature never adds a
    /// role the operator did not request.
    #[test]
    fn feature_flags_gate_roles_without_adding_any() {
        let mut cli = CliConfig::default();
        cli.auth.jwt_secret = Some("a".repeat(32));
        cli.features.realtime = false;

        let roles = ServerRoles::parse(&["api".to_string(), "realtime".to_string()]).unwrap();
        let settings = settings_for("127.0.0.1".into(), 0, roles, &cli).unwrap();
        let gated =
            ferris_server::server::gate_roles(&settings.roles, &settings.config.features).unwrap();
        assert!(gated.has(ServerRole::Api));
        assert!(
            !gated.has(ServerRole::Realtime),
            "feature realtime=false must remove the role"
        );
        assert!(!gated.has(ServerRole::Web), "no feature may add a role");

        // And with every feature on, the same single role stays single.
        let mut all_on = settings.config.features.clone();
        all_on.realtime = true;
        let gated = ferris_server::server::gate_roles(&settings.roles, &all_on).unwrap();
        assert!(gated.has(ServerRole::Realtime));
        assert!(!gated.has(ServerRole::Worker));
        assert!(!gated.has(ServerRole::Scheduler));
    }

    /// Every `[features]` gate off, so a test can use an `api`-only build
    /// without the default flags adding unrelated mismatches.
    fn quiet_gates(cli: &mut CliConfig) {
        cli.features.realtime = false;
        cli.features.jobs = false;
        cli.features.scheduler = false;
        cli.features.email = false;
        cli.features.files = false;
        cli.features.print = false;
        cli.features.report = false;
        cli.features.webhooks = false;
    }

    #[test]
    fn maps_cli_database_config() {
        for (raw, expected) in [
            ("postgres", DatabaseType::Postgres),
            ("PostgreSQL", DatabaseType::Postgres),
            ("mysql", DatabaseType::MySQL),
            ("sqlite", DatabaseType::SQLite),
        ] {
            let cli = DatabaseConfig {
                r#type: raw.into(),
                host: "localhost".into(),
                port: 5432,
                name: "db".into(),
                user: "u".into(),
                password: "p".into(),
                max_connections: 5,
                ssl_mode: "prefer".into(),
            };
            assert_eq!(database_config(&cli).unwrap().r#type, expected, "for {raw}");
        }
    }

    /// `[apps]` used to be dropped on the floor here: `build_core_config` wrote
    /// `apps: Default::default()`, so the server never saw the operator's
    /// directory even after the CLI learned to read it.
    #[test]
    fn maps_apps_config_into_the_core_config() {
        let base = CliConfig::default();
        let cli = CliConfig {
            auth: crate::config::AuthConfig {
                jwt_secret: Some("a".repeat(32)),
                ..base.auth.clone()
            },
            apps: ferris_core::AppsConfig {
                wasm_dir: std::path::PathBuf::from("apps"),
                enabled: vec!["my-app".to_string()],
                ..ferris_core::AppsConfig::default()
            },
            ..base
        };

        let core = build_core_config(&cli).unwrap();
        assert_eq!(core.apps.wasm_dir, std::path::PathBuf::from("apps"));
        assert_eq!(core.apps.enabled, vec!["my-app".to_string()]);
    }

    /// `ferris dev` publishes the artifact it just built and hands the store to
    /// the server; booting `[apps]` on top would load a second, possibly stale
    /// copy of the same app.
    ///
    /// `wasm`-gated because the store and `ServerSettings::plugins` only exist
    /// when the plugin host is compiled in; the api-only build has no test to
    /// run here.
    #[cfg(feature = "wasm")]
    #[tokio::test]
    async fn a_process_that_already_published_a_store_is_not_bootstrapped_again() {
        let mut settings = ServerSettings::new(
            "127.0.0.1".into(),
            0,
            ServerRoles::parse(&["api".to_string()]).unwrap(),
            ferris_core::DatabaseConfig::default(),
            ferris_core::Config::default(),
        );
        assert!(
            needs_plugin_boot(&settings),
            "serve/run start with no store and must publish [apps]"
        );

        let pool = ferris_db::memory_db().await.unwrap();
        settings.plugins = Some(super::super::plugin_boot::new_store(pool, None).unwrap());
        assert!(
            !needs_plugin_boot(&settings),
            "an already published store must not be replaced"
        );
    }

    /// `[server] allowed_hosts` is not a field of the CLI's `ServerConfig`, so
    /// the one place that must not lose it is the reader used at boot. Its own
    /// tests live next to the reader, in `commands::site`.
    #[test]
    fn the_allowed_hosts_reader_is_the_sites() {
        let path = temp_config("[server]\nallowed_hosts = [\"erp.example.com\"]\n");
        assert_eq!(
            super::super::site::allowed_hosts_from_file(&path).unwrap(),
            vec!["erp.example.com".to_string()]
        );
        let _ = std::fs::remove_file(path);
    }

    fn temp_config(contents: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("ferris-hosts-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&path, contents).expect("the temporary config is writable");
        path
    }
}
