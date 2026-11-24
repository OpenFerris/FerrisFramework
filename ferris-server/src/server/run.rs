//! The unified server: boot sequence, listener lifecycle and shutdown.
//!
//! # Boot order and the feature matrix
//!
//! 1. Publish what this binary compiled ([`crate::host_features`]) so the WIT
//!    layer and the doctor see it.
//! 2. Refuse a configuration that asks for a subsystem the binary lacks
//!    ([`validate_compiled_features`]) — before any pool, listener or task is
//!    created.
//! 3. Reduce the requested roles by `[features]` and by what was compiled
//!    ([`gate_roles`]).
//! 4. Boot the rest in dependency order: PKI → auth → authz → keyring → schema →
//!    seeds → email hub → integrations → router → plugin network → background
//!    roles.
//!
//! Steps that belong to an optional vertical are behind its `#[cfg(feature)]`
//! and are simply absent from a build without it; the sequence above stays
//! readable because each absence has one obvious reason.

use std::future::IntoFuture;
use std::sync::Arc;

use anyhow::Result;
use ferris_api::SessionHandlers;
use ferris_auth::{ApiKeyManager, AuthState, JwtManager, SessionConfig, SessionManager, UserStore};
#[cfg(feature = "jobs")]
use ferris_jobs::WorkerPool;
use tracing::{info, warn};

#[cfg(feature = "wasm")]
use crate::plugin_net::PluginNetwork;
use crate::roles::ServerRole;
use crate::router::build_router;
use crate::shutdown::{serve_with_drain, shutdown_signal};
use crate::tls::{
    CertificateRevocation, CombinedRevocation, TlsClientInfo, TlsListener, build_server_config,
};

#[cfg(feature = "wasm")]
use super::boot::start_plugin_clock;
use super::boot::{bootstrap_authz, install_secret_keyring, jwt_secret};
use super::gating::{gate_roles, validate_compiled_features};
use super::settings::ServerSettings;

/// The unified server.
#[derive(Debug)]
pub struct Server {
    settings: ServerSettings,
}

impl Server {
    pub fn new(settings: ServerSettings) -> Self {
        Self { settings }
    }

    pub fn settings(&self) -> &ServerSettings {
        &self.settings
    }

    /// Start serving. Blocks until shutdown.
    pub async fn run(self) -> Result<()> {
        let mut settings = self.settings;

        // What this binary can serve, published before anything asks. A second
        // install in the same process is a programming error, not a reason to
        // panic: report it and keep the first set.
        if !crate::host_features::install_compiled() {
            warn!(
                "host features were already installed; keeping the first set \
                 (compiled features: `{}`)",
                crate::host_features::installed()
            );
        }

        // A config that asks for a subsystem this binary lacks is a boot error,
        // never a silent no-op: the process would otherwise report itself
        // healthy while serving less than its configuration declares.
        validate_compiled_features(&settings.config)?;

        // `[features]` is authoritative over `--all`/`server.roles`: a disabled
        // subsystem must not start just because the role was requested. A role
        // whose Cargo feature is missing is removed the same way, with the
        // rebuild hint instead of the feature flag.
        settings.roles = gate_roles(&settings.roles, &settings.config.features)?;
        if settings.roles.is_empty() {
            anyhow::bail!(
                "every requested role was disabled by [features] or was not compiled into this \
                 binary; enable realtime/jobs/scheduler in the config, request the api/web \
                 role, or rebuild with the matching `--features`"
            );
        }
        settings.validate()?;
        let settings = &settings;

        for role in settings.roles.iter() {
            info!("role enabled: {}", role.as_str());
        }

        let pool = match &settings.pool {
            Some(pool) => pool.clone(),
            None => ferris_db::create_pool(&settings.database).await?,
        };

        // §C4: the WASM CPU cap for app plugins only exists while a clock keeps
        // bumping the engine's epoch. The host arms an epoch deadline around
        // every plugin call, and `start_plugin_clock` is what fires it; before
        // C4 no production path started that clock, so a plugin in an infinite
        // loop hung its worker forever while looking protected.
        //
        // Started *before* anything can call into a plugin — `build_router` runs
        // each app's `describe()` to register its methods and routes, which is
        // already guest code. Starting it after the router left that boot-time
        // work uncapped.
        #[cfg(feature = "wasm")]
        let epoch_clock = start_plugin_clock(settings.plugins.as_ref());

        // Phase 3 / H2: load the internal CA when configured and register it as
        // the process-wide `pki` provider *before* anything can call the plugin
        // imports. The same handle doubles as the TLS revocation store (below),
        // so a certificate revoked through the plugin is refused at the next
        // handshake rather than merely by a later permission check.
        //
        // Without the `wasm` feature the CA still loads: it is the revocation
        // store for mTLS, which an API-only deployment can use on its own. Only
        // the guest-facing provider registration is compiled out.
        let pki = match settings.config.server.pki.as_ref() {
            Some(cfg) => {
                let pki = crate::pki::HostPki::load(cfg, pool.clone()).await?;
                #[cfg(feature = "wasm")]
                if !ferris_wasm::set_pki_provider(pki.clone()) {
                    anyhow::bail!("a pki provider was already registered");
                }
                info!(
                    "pki: internal CA loaded from {:?} (default TTL {}s)",
                    cfg.ca_cert_path, cfg.default_ttl_seconds
                );
                Some(pki)
            }
            None => None,
        };

        // Build AuthState for authentication middleware. The session manager
        // shares the database so sessions survive restarts and are visible to
        // the public login endpoint.
        //
        // The Desk can authenticate with DB-backed session cookies alone, but
        // the JWT manager is still part of `AuthState` (API tokens, plugin
        // calls), so a missing secret is an actionable boot error naming the
        // variable — not the raw `JwtConfig::new("")` failure from inside
        // `ferris-auth` (SRV16).
        let jwt_secret = jwt_secret(&settings.config)?;
        let jwt_manager = JwtManager::new(ferris_auth::JwtConfig::new(jwt_secret)?)?;
        // With TLS termination configured, session cookies are marked `Secure`
        // so browsers never send them over a plaintext hop. Without it the Desk
        // runs over plain HTTP in dev, where a `Secure` cookie would be refused
        //.
        let session_config = SessionConfig {
            secure_cookies: settings.config.server.tls.is_some(),
            max_sessions_per_user: settings.config.auth.max_sessions_per_user,
            ..SessionConfig::default()
        };
        let session_manager = SessionManager::with_db(session_config.clone(), pool.clone());
        session_manager.init().await?;
        let api_key_manager = ApiKeyManager::with_db(pool.clone());
        api_key_manager.init().await?;
        let auth_state =
            AuthState::new(jwt_manager, session_manager.clone(), session_config.clone())
                // Resolve users (roles included) from the users table on every request
                // so REST writes authorise as the real caller with real roles.
                .with_user_validator(Arc::new(UserStore::new(pool.clone())))
                // Accept the Frappe `Authorization: token <key>:<secret>` scheme
                // (AUTH2). Keys are documents, so the same table backs issue and
                // verify.
                .with_api_keys(Arc::new(api_key_manager));
        let session_handlers = SessionHandlers::new(session_config, session_manager, pool.clone())
            // Wire the login gate + MFA self-service to the operator's policy
            // (`max_login_attempts`, `lockout_duration_minutes`,
            // `mfa_enabled`, `mfa_issuer`).
            .with_auth_config(&settings.config.auth);

        // Native modules: the registry is built before the authz bootstrap so
        // the DocTypes declared by optional verticals are part of the same
        // policy install. Boot happens after the shared services exist.
        let modules = crate::router::modules::native_modules(&settings.config.features);

        // Permission policies, before anything can serve a request.
        //
        // Order matters and is the reason this is not inside `build_router`:
        // only DocTypes already in the registry are translated, and the framework
        // ones are registered eagerly by `register_core_doctypes`, so registration
        // has to happen first. `build_router` calls it too — it is idempotent —
        // but by then the engine would already exist with the core DocTypes
        // missing from it. The pool makes the persisted Role Permissions rows
        // part of the same rebuild.
        let policies = bootstrap_authz(&pool, settings.policy_path.as_deref(), &modules)
            .await
            .map_err(|e| anyhow::anyhow!("authz bootstrap failed: {e}"))?;
        info!(
            "authz: {} DocTypes registered, {} with declared rules, {} statements installed, {} rules skipped",
            policies.doctypes,
            policies.doctypes_with_rules,
            policies.statements,
            policies.skipped.len()
        );
        for line in policies.skipped_lines() {
            warn!("authz: skipped a declared rule — {line}");
        }

        // `Secret` fields (H3) need their AEAD keyring installed before any
        // connector can load a credential. The catalogue is registered by now,
        // which is what lets the warning name the real situation instead of
        // guessing.
        install_secret_keyring(&settings.config)?;

        // Mirror the schema registry into `tabDocType` — the DocType DocType —
        // so the schema is queryable through the ordinary engine, next to the
        // Singles. Never fatal: a bad declaration is logged, like a failed seed.
        let synced = ferris_document::DoctypeStore::new(&pool)
            .sync_registry()
            .await;
        info!(
            "schema: {} DocTypes stored in the DocType table",
            synced.synced.len()
        );
        for line in &synced.warnings {
            warn!("schema: could not store a DocType — {line}");
        }

        // Materialise the Single DocTypes (the install-wide settings) with their
        // declared defaults, before any host logic reads them. This is what lets
        // the global search, the scheduler and the web shell read real values
        // instead of an empty row, and what the Desk opens on.
        let seeds = ferris_api::seed_singletons(&pool).await;
        info!(
            "settings: {} Single DocTypes materialised",
            seeds.seeded.len()
        );
        for line in &seeds.warnings {
            warn!("settings: could not seed a Single — {line}");
        }

        // The reference rows every `Link` points at: one `Module Def` per
        // declared module and one `Language` per advertised locale. Without
        // them the workspace module links dangle and the language pickers open
        // empty on a fresh install.
        let languages: Vec<String> = settings
            .i18n
            .supported()
            .iter()
            .map(|locale| locale.as_str().to_string())
            .collect();
        let reference = ferris_api::seed_reference_data(&pool, &languages).await;
        info!(
            "reference: {} module(s), {} language(s) seeded",
            reference.modules, reference.languages
        );
        for line in &reference.warnings {
            warn!("reference: could not seed a row — {line}");
        }

        // Audit sink: the events are emitted by `ferris-api` (login/logout) and
        // `ferris-files` (file reads), which cannot depend on the host. The
        // sink spawns the write so a slow insert never delays the operation it
        // describes; without it, `record` is a no-op.
        let audit_pool = pool.clone();
        let installed = ferris_core::audit::set_sink(std::sync::Arc::new(move |event| {
            let pool = audit_pool.clone();
            let event = event.clone();
            tokio::spawn(async move {
                match event {
                    ferris_core::audit::AuditEvent::Login { user, ip } => {
                        crate::observability::record_activity(
                            &pool,
                            "login",
                            &user,
                            &ip,
                            "Signed in",
                        )
                        .await;
                    }
                    ferris_core::audit::AuditEvent::Logout { user, ip } => {
                        crate::observability::record_activity(
                            &pool,
                            "logout",
                            &user,
                            &ip,
                            "Signed out",
                        )
                        .await;
                    }
                    ferris_core::audit::AuditEvent::FileAccess {
                        user,
                        file_type,
                        reference,
                    } => {
                        crate::observability::record_access(&pool, &user, &file_type, &reference)
                            .await;
                    }
                }
            });
        }));
        if !installed {
            warn!("audit: a sink is already installed; activity rows go to that one");
        }

        // `System Settings` is the install-wide switchboard. Read after the seed,
        // so the row (with its `enable_scheduler` default of `1`) is guaranteed
        // to exist and the host never has to treat "not yet materialised" as a
        // third state. Only the scheduler role reads it.
        #[cfg(feature = "scheduler")]
        let scheduler_enabled = ferris_api::DocStore::get_single_system(&pool, "System Settings")
            .await
            .ok()
            .and_then(|settings| {
                settings
                    .get("enable_scheduler")
                    .and_then(ferris_core::Value::as_bool)
            })
            .unwrap_or(true);

        // Email transport: build once when any background role is enabled, so
        // the email worker and the integrations runtime's mailer drain the
        // same `email` queue. A disabled `email` feature wins over the role:
        // the hub is not built and no worker is started.
        #[cfg(feature = "email")]
        let email_hub = {
            let has_background_role =
                settings.roles.has(ServerRole::Worker) || settings.roles.has(ServerRole::Scheduler);
            if has_background_role && !settings.config.features.email {
                warn!(
                    "feature `email` is disabled in [features]: the email hub and worker will not start"
                );
            }
            if has_background_role && settings.config.features.email {
                Some(crate::background::build_email_hub(settings, &pool).await?)
            } else {
                None
            }
        };

        // The default job queue is built before the modules boot, so the
        // `QueuePort` handle is available to a module's `boot` and not only to
        // the worker/scheduler started later.
        #[cfg(feature = "jobs")]
        let app_queue = if settings.roles.has(ServerRole::Worker)
            || settings.roles.has(ServerRole::Scheduler)
        {
            Some(crate::background::build_default_queue(settings, &pool).await?)
        } else {
            None
        };

        // `print`, `report` and `search` are enforced by the API router (the
        // routes are not mounted) and `webhooks` by the integrations runtime's
        // document hooks; no separate boot warning is needed here.
        // Integrations runtime: mount the document-event hooks (webhooks,
        // automation, assignment, milestones). `bootstrap_authz` above has
        // registered the integration DocTypes, so every hook target resolves;
        // `install` is idempotent and re-running it later covers DocTypes that
        // appear after boot.
        #[cfg(all(feature = "integrations", not(feature = "email")))]
        let integrations_state =
            ferris_integrations::runtime::IntegrationsContext::new(pool.clone())
                // `webhooks = false` skips only webhook dispatch; automation,
                // assignment, milestones and push keep running (the hook layer
                // reads the flag).
                .with_features(settings.config.features.clone());
        // The mailer bridge needs both halves: an email queue to enqueue on and
        // the integrations runtime to call it.
        #[cfg(all(feature = "integrations", feature = "email"))]
        let integrations_state =
            ferris_integrations::runtime::IntegrationsContext::new(pool.clone())
                .with_features(settings.config.features.clone())
                .with_mailer(crate::background::integrations_mailer(email_hub.as_ref()));
        // The dispatcher takes its own copy of the context: `install` consumes
        // one, the worker registers handlers from the other. Both exist only when
        // both features do, so a build without a background role consumes the
        // single copy instead of cloning into a value it then drops.
        #[cfg(all(feature = "integrations", feature = "jobs"))]
        let integrations_worker = integrations_state.clone();
        #[cfg(feature = "integrations")]
        ferris_integrations::runtime::install(integrations_state);

        // Tags: mount the document-event hooks that keep the `Tag` catalogue
        // and the `Tag Link` associations in step with every saved document's
        // standard `tags` field. Registered here, after the catalog above, and
        // idempotent like the integrations install so a hot reload or a plugin
        // that adds DocTypes can re-run it.
        ferris_api::tags::install(pool.clone());

        let handles = ferris_core::ports::ServiceHandles::new();
        #[cfg(feature = "jobs")]
        if let Some(queue) = &app_queue {
            handles.install_queue(std::sync::Arc::new(ferris_jobs::ports::JobQueuePort::new(
                queue.clone(),
            )));
        }
        #[cfg(feature = "email")]
        if let Some(hub) = &email_hub {
            if let Some(mailer) = crate::background::email_hub_mailer(hub) {
                handles.install_mailer(std::sync::Arc::new(mailer));
            }
        }

        let service_ctx = ferris_api::ServiceContext {
            db: pool.clone(),
            auth: auth_state.clone(),
            config: std::sync::Arc::new(settings.config.clone()),
            features: settings.config.features.clone(),
            roles: ferris_api::ServiceRoles {
                api: settings.roles.has(ServerRole::Api),
                web: settings.roles.has(ServerRole::Web),
                realtime: settings.roles.has(ServerRole::Realtime),
                worker: settings.roles.has(ServerRole::Worker),
                scheduler: settings.roles.has(ServerRole::Scheduler),
            },
            handles,
        };
        modules
            .boot(&service_ctx)
            .await
            .map_err(|error| anyhow::anyhow!("native module boot failed: {error}"))?;

        let bg_pool = pool.clone();
        // The plugin network needs the same `AuthState` the router gates sessions
        // with; `build_router` consumes its copy below.
        #[cfg(feature = "wasm")]
        let auth_state_for_network = auth_state.clone();
        let app = build_router(settings, pool, auth_state, session_handlers, &modules);

        // The sockets an app declared. Registered before the listener
        // accepts, so an `io::connect-stream` from a request never finds the
        // service missing. The transport owns outbound TCP/TLS streams, the
        // listeners declared as `%stream` routes, and the dedicated TLS listener
        // that serves `tls-server` routes with the app's own certificate.
        #[cfg(feature = "wasm")]
        let _plugin_network = match settings.plugins.clone() {
            Some(plugins) => {
                PluginNetwork::start(settings, &plugins, &auth_state_for_network).await
            }
            None => PluginNetwork::default(),
        };

        // Mirror the compiled workspace registry into `tabWorkspace` — the
        // Workspace DocType — so the desk is backed by documents the Desk can
        // list and inspect, not by the in-memory registry alone. Must run after
        // `build_router` registered the workspace catalog. Never fatal: a bad
        // declaration is logged, like a failed schema sync.
        let seeded_workspaces = ferris_api::seed_workspaces(&bg_pool).await;
        info!(
            "workspaces: {} stored in the Workspace table",
            seeded_workspaces.seeded.len()
        );
        for line in &seeded_workspaces.warnings {
            warn!("workspaces: could not store a Workspace — {line}");
        }

        // Background roles own their tasks for the whole server lifetime; they
        // are shut down after the listener stops.
        #[cfg(feature = "jobs")]
        let mut worker_pool: Option<WorkerPool> = None;
        // SRV7: the observability snapshot needs the same queue and scheduler
        // the background roles built, but only for reading.
        #[cfg(feature = "jobs")]
        let mut observability_queue = None;
        #[cfg(feature = "scheduler")]
        let mut scheduler: Option<Arc<ferris_scheduler::Scheduler>> = None;

        #[cfg(feature = "jobs")]
        if settings.roles.has(ServerRole::Worker) || settings.roles.has(ServerRole::Scheduler) {
            // SRV4: the email hub is built for **either** background role, so
            // its consumer must run for either role too. Before this, a
            // scheduler-only process accepted integrations mail into a queue
            // nothing drained, and reported success.
            let mut combined = WorkerPool::new();
            #[cfg(feature = "email")]
            if let Some(hub) = email_hub.as_ref() {
                combined = crate::background::start_email_worker(settings, hub);
            }
            // SRV10: one default queue shared by the app worker and the
            // scheduler, instead of each building its own client and `init()`.
            let app_queue = app_queue
                .clone()
                .expect("the background role built the default queue above");
            observability_queue = Some(app_queue.clone());
            // A14: any process that can *produce* jobs (worker or scheduler)
            // also *consumes* the default queue, or scheduled jobs accumulate
            // forever. The plain worker role gets it alongside the email one;
            // the scheduler role becomes self-contained like Frappe's.
            let app_worker = crate::background::start_app_worker(
                settings,
                app_queue.clone(),
                #[cfg(feature = "integrations")]
                integrations_worker,
                #[cfg(feature = "wasm")]
                settings.plugins.clone(),
            )
            .await?;
            combined.add_worker(app_worker);
            worker_pool = Some(combined);

            // The scheduler role is gated by `System Settings.enable_scheduler`:
            // an operator can stop cron jobs from the Desk without relaunching
            // the process under a different role set.
            #[cfg(feature = "scheduler")]
            {
                let scheduler_role = settings.roles.has(ServerRole::Scheduler);
                if scheduler_role && !scheduler_enabled {
                    warn!(
                        "scheduler role enabled but System Settings.enable_scheduler is off — \
                         no scheduled jobs will run"
                    );
                }
                if scheduler_role && scheduler_enabled {
                    scheduler = Some(
                        crate::background::start_scheduler(settings, &bg_pool, app_queue).await?,
                    );
                }
            }
        }

        // SRV7: the state-driven producers run for the serve lifetime even in a
        // serve-only process, because the health/storage/version snapshots and
        // the prepared-report mirror do not depend on a background role.
        let sources = crate::observability::Sources {
            #[cfg(feature = "jobs")]
            queue: observability_queue,
            #[cfg(feature = "scheduler")]
            scheduler: scheduler.clone(),
        };
        let observability = crate::observability::spawn(bg_pool.clone(), sources);

        let listener = tokio::net::TcpListener::bind((settings.host.as_str(), settings.port))
            .await
            .map_err(|e| {
                anyhow::anyhow!("failed to bind {}:{}: {}", settings.host, settings.port, e)
            })?;
        let addr = listener.local_addr()?;
        let (shutdown_started_tx, mut shutdown_started_rx) = tokio::sync::watch::channel(false);

        // Terminate TLS in-process when configured. The client identity travels
        // as `ConnectInfo<TlsClientInfo>`, so the identity is attached to the
        // connection the moment the handshake completes — before any route runs.
        match settings.config.server.tls.as_ref() {
            Some(tls) => {
                // Combine the internal CA's ledger with any operator-supplied
                // store: both must be able to refuse a handshake, and neither
                // should silently override the other.
                let mut revocation_stores: Vec<Arc<dyn CertificateRevocation>> = Vec::new();
                if let Some(pki) = &pki {
                    revocation_stores.push(pki.clone());
                }
                if let Some(operator) = &settings.revocation {
                    revocation_stores.push(operator.clone());
                }
                let revocation: Option<Arc<dyn CertificateRevocation>> =
                    if revocation_stores.is_empty() {
                        None
                    } else {
                        Some(Arc::new(CombinedRevocation::new(revocation_stores)))
                    };
                let tls_config = build_server_config(tls, revocation)?;
                let listener = TlsListener::new(listener, tls_config);
                info!("ferris server listening on {addr} (https)");
                let serve = axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<TlsClientInfo>(),
                )
                .with_graceful_shutdown(shutdown_signal(shutdown_started_tx.clone()))
                .into_future();
                serve_with_drain(
                    serve,
                    &mut shutdown_started_rx,
                    crate::shutdown::DRAIN_TIMEOUT,
                )
                .await?;
            }
            None => {
                info!("ferris server listening on {addr}");
                // SEC-08: without `ConnectInfo` the audit row has no peer and
                // every forwarding header is (correctly) ignored. Plain HTTP
                // still needs the real peer so a trusted proxy can be
                // distinguished from a direct client.
                let serve = axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                .with_graceful_shutdown(shutdown_signal(shutdown_started_tx.clone()))
                .into_future();
                serve_with_drain(
                    serve,
                    &mut shutdown_started_rx,
                    crate::shutdown::DRAIN_TIMEOUT,
                )
                .await?;
            }
        }

        // Stop the WASM epoch clock and the observability snapshot with the
        // listener.
        #[cfg(feature = "wasm")]
        if let Some(clock) = epoch_clock {
            clock.abort();
        }
        observability.abort();

        // Shut the background roles down once the listener stops.
        #[cfg(feature = "jobs")]
        if let Some(mut worker_pool) = worker_pool {
            worker_pool.shutdown().await;
        }
        #[cfg(feature = "scheduler")]
        if let Some(scheduler) = scheduler {
            scheduler.shutdown().await?;
        }
        modules.shutdown(&service_ctx).await;

        Ok(())
    }
}
