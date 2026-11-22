//! Background service tasks: real worker and scheduler wired to the server roles.
//!
//! The worker role runs the `email` queue consumer with a `job_workers`
//! heartbeat (the consumer itself lives in the `email` submodule); the scheduler role runs
//! persisted cron schedules and enqueues their jobs on the shared queue.
//!
//! # Compile-time gating
//!
//! The module is compiled with the `jobs` feature (the base every background
//! role needs: the queue, the worker pool and the dispatcher). Inside it, each
//! consumer is gated by the vertical it serves:
//!
//! * `email` — the hub, the email worker and the integrations mailer;
//! * `scheduler` — cron loading and the seeded integration sweeps;
//! * `integrations` — the dispatcher handlers and the sweep seeds;
//! * `wasm` — the plugin `handle-job` fallback the app worker routes to.
//!
//! A build without one of them simply has no consumer for it: the queue is
//! still there (jobs is on), which is what keeps `--no-default-features
//! --features api,jobs` coherent.

use anyhow::Result;
use ferris_db::Db;
#[cfg(feature = "scheduler")]
use ferris_jobs::JobPriority;
use ferris_jobs::{JobDispatcher, JobQueue, QueueConfig, Worker, WorkerConfig};
#[cfg(feature = "scheduler")]
use ferris_scheduler::{Schedule, ScheduleConfig, Scheduler};
use redis::Client as RedisClient;
use std::sync::Arc;
use tracing::{info, warn};

use crate::ServerSettings;
#[cfg(all(feature = "jobs", feature = "integrations"))]
use crate::roles::ServerRole;

#[cfg(feature = "email")]
mod email;
#[cfg(feature = "email")]
pub use email::*;

#[cfg(all(test, feature = "jobs", feature = "integrations"))]
mod tests;

/// Open the Redis client from server settings.
fn redis_client(settings: &ServerSettings) -> Result<RedisClient> {
    RedisClient::open(settings.config.redis.url.as_str())
        .map_err(|e| anyhow::anyhow!("invalid redis url: {}", e))
}

/// Build the process's job queue: the Redis sorted set when a server is
/// reachable, otherwise a database-only queue.
///
/// The durable `jobs` table already backs every queue (it is what `pop_job`
/// claims from); Redis only accelerates the enqueue hot path. When no Redis is
/// configured or reachable, the scheduler and workers fall back to the database
/// (SQLite on a single-node/edge install) instead of refusing to start
/// (R-FW-1a).
async fn build_queue(settings: &ServerSettings, pool: &Db, config: QueueConfig) -> JobQueue {
    match redis_client(settings) {
        Ok(client) => match JobQueue::new(config.clone(), client).await {
            Ok(queue) => return queue.with_db(pool.clone()),
            Err(e) => warn!("redis queue unavailable ({e}); using the database-backed queue"),
        },
        Err(e) => warn!("redis not configured ({e}); using the database-backed queue"),
    }
    JobQueue::new_db(config, pool.clone())
}

/// Build and initialise the **default** job queue once per process (SRV10).
///
/// The app worker and the scheduler used to call `build_queue` independently,
/// so both opened their own Redis client and ran the queue DDL for the same
/// queue name. Sharing one `Arc<JobQueue>` keeps a single connection state and a
/// single `init()` while preserving the database fallback (R-FW-1a).
///
/// # Errors
///
/// Fails only when the queue's schema cannot be initialised.
pub async fn build_default_queue(settings: &ServerSettings, pool: &Db) -> Result<Arc<JobQueue>> {
    let jobs_cfg = &settings.config.jobs;
    let queue = build_queue(
        settings,
        pool,
        QueueConfig {
            name: jobs_cfg.default_queue.clone(),
            retry_backoff_base_secs: jobs_cfg.retry_backoff_base_secs,
            dead_letter_queue: jobs_cfg.dead_letter_queue.clone(),
        },
    )
    .await;
    queue.init().await?;
    Ok(Arc::new(queue))
}

/// Start the generic application worker on the scheduler's default queue (A14).
///
/// The scheduler (and ordinary app code) enqueues jobs carrying a `job_name`,
/// and before A14 nothing consumed that queue: scheduled work sat `pending`
/// forever while looking queued, and a restart flushed it silently. A
/// [`JobDispatcher`] routes each claimed job to the handler registered for its
/// name and fails loudly — retrying, then dead-lettering — when no handler
/// exists, instead of never running it.
///
/// `integrations` and `plugins` are compile-time optional: each one is present
/// only when its feature is, and `run` passes it only then. Without either, the
/// dispatcher starts empty and a claimed job fails loudly as "no handler",
/// which is the honest state of a minimal build rather than a worker that
/// pretends to dispatch.
///
/// # Errors
///
/// Fails only when the queue's schema cannot be initialised; the queue falls
/// back to the database when Redis is absent (R-FW-1a).
pub async fn start_app_worker(
    settings: &ServerSettings,
    queue: Arc<JobQueue>,
    #[cfg(feature = "integrations")]
    integrations: ferris_integrations::runtime::IntegrationsContext,
    #[cfg(feature = "wasm")] plugins: Option<Arc<ferris_wasm::PluginStore>>,
) -> Result<Worker<JobDispatcher>> {
    let jobs_cfg = &settings.config.jobs;

    // The integrations runtime's sweeps (auto repeat, reminders, auto email
    // reports, push delivery) are app-queue jobs; register their handlers so
    // a `job_name` the scheduler enqueues actually runs instead of failing
    // loudly as "no handler".
    let mut dispatcher = JobDispatcher::new();
    #[cfg(feature = "integrations")]
    {
        // A scheduler-only or scheduler+worker process enqueues
        // `integrations_push_delivery` every minute from the seeded schedule,
        // so it must not also run the sweep inline (two sweeps would race for
        // the same `Queued` rows). A worker-only process has no scheduler to
        // enqueue anything, so it owns the sweep directly.
        let sweep_state = integrations.clone();
        ferris_integrations::runtime::register_handlers(&mut dispatcher, integrations);
        if push_sweep_is_inline(settings) {
            spawn_push_sweep(sweep_state);
        }
    }
    // Route any job the integration handlers do not own to the loaded plugin's
    // `handle-job` export, so a schedule's `job_name` actually re-enters the
    // guest instead of failing as "no handler". The plugin side has no
    // fallback: a claim while no plugin is loaded still fails loudly.
    #[cfg(feature = "wasm")]
    if let Some(plugins) = plugins {
        dispatcher.set_fallback(PluginJobHandler { plugins });
    }

    let worker = Worker::new(
        WorkerConfig {
            worker_id: format!("app-{}", uuid::Uuid::new_v4()),
            queues: vec![jobs_cfg.default_queue.clone()],
            concurrency: jobs_cfg.worker_concurrency.max(1),
            poll_interval_ms: jobs_cfg.poll_interval_ms,
            shutdown_timeout_secs: 30,
        },
        dispatcher,
        queue,
    );
    info!(
        "app worker started on queue `{}` ({} slots)",
        jobs_cfg.default_queue,
        jobs_cfg.worker_concurrency.max(1)
    );
    Ok(worker)
}

/// Cadence of the in-process push sweep. The seeded scheduler job uses the
/// same one minute; the two are mutually exclusive by role.
#[cfg(all(feature = "jobs", feature = "integrations"))]
const PUSH_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Whether this process must run the push sweep itself.
///
/// A process with the `scheduler` role gets `integrations_push_delivery` from
/// the seeded schedule; a worker-only process has no scheduler to enqueue it,
/// so it sweeps inline. The predicate exists so the rule is testable without
/// spawning tasks.
#[cfg(all(feature = "jobs", feature = "integrations"))]
fn push_sweep_is_inline(settings: &ServerSettings) -> bool {
    !settings.roles.has(ServerRole::Scheduler)
}

/// Sweep `Push Notification` rows until the process shuts down.
///
/// One task per process, started by [`start_app_worker`] only when the
/// scheduler role is absent. Delivery is the same `deliver_pending` the job
/// handler runs, so the retry/backoff rules are identical either way.
#[cfg(all(feature = "jobs", feature = "integrations"))]
fn spawn_push_sweep(state: ferris_integrations::runtime::IntegrationsContext) {
    let mut shutdown = ferris_api::shutdown::subscribe();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(PUSH_SWEEP_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    match ferris_integrations::runtime::push::deliver_pending(&state).await {
                        Ok(0) => {}
                        Ok(delivered) => info!(delivered, "in-app push sweep delivered"),
                        Err(error) => warn!(error = %error, "in-app push sweep failed"),
                    }
                }
            }
        }
    });
}

/// Routes a claimed job the integration handlers do not own to the loaded
/// plugin's `handle-job` export.
///
/// This is the callback a cron schedule needs to re-enter the guest: before it,
/// a schedule could be registered but nothing ever ran the app's code for it.
/// Installed as the app dispatcher's fallback (exact-name handlers win), so the
/// `job_name` a schedule enqueues is offered to the app as its `job-type`.
#[cfg(feature = "wasm")]
struct PluginJobHandler {
    plugins: Arc<ferris_wasm::PluginStore>,
}

#[cfg(feature = "wasm")]
#[async_trait::async_trait]
impl ferris_jobs::JobHandler for PluginJobHandler {
    type Payload = ferris_core::Value;
    type Output = ferris_core::Value;

    fn job_name(&self) -> &'static str {
        // Only ever installed as the dispatcher's fallback; this name is not
        // used for exact-match routing.
        "plugin"
    }

    async fn handle(
        &self,
        payload: ferris_core::Value,
        ctx: ferris_jobs::JobContext,
    ) -> ferris_core::Result<ferris_core::Value> {
        // `scheduler::enqueue` stores the payload as parsed JSON when it parses,
        // otherwise as the raw string; serializing the `Value` back keeps the
        // guest's `handle-job(job-type, payload)` contract (a JSON body).
        let payload = serde_json::to_string(&payload).unwrap_or_else(|_| payload.to_string());
        let out = self
            .plugins
            .handle_job(&ctx.job_name, &payload)
            .map_err(|e| ferris_core::Error::Job(ferris_core::JobError::Failed(e.to_string())))?;
        Ok(ferris_core::Value::String(out))
    }
}

/// Start the scheduler role: load persisted schedules and run their cron jobs.
///
/// Takes the process's shared default queue (SRV10) so the scheduler and the
/// app worker claim from the same `Arc<JobQueue>` instead of each building one.
/// # Errors
///
/// Fails when the scheduler cannot be built (an invalid `timezone`, or a table
/// cannot be prepared), when a seeded integration schedule cannot be persisted,
/// or when `start` cannot begin. A failed lease attempt is not an error: the
/// instance serves reads and retries in the background.
#[cfg(feature = "scheduler")]
pub async fn start_scheduler(
    settings: &ServerSettings,
    pool: &Db,
    queue: Arc<JobQueue>,
) -> Result<Arc<Scheduler>> {
    let jobs_cfg = &settings.config.jobs;
    let sched_cfg = &settings.config.scheduler;

    // Production must build from the operator's `[scheduler]` table, not from
    // the defaults: `tick_interval_secs`, `max_concurrent_jobs` and
    // `history_retention_days` were previously parsed, logged and ignored
    // (SCHED-01). An invalid `timezone` fails here instead of silently
    // scheduling in UTC.
    let scheduler = Scheduler::with_config(queue, pool.clone(), sched_cfg).await?;

    // Seed the integrations runtime's sweep schedules. Idempotent by
    // `job_name`: a schedule an operator has already created (often with its
    // own cadence) wins; the seeds only fill gaps so a fresh install starts
    // executing the sweeps without further configuration.
    #[cfg(feature = "integrations")]
    seed_integration_schedules(&scheduler, jobs_cfg.default_queue.as_str()).await?;

    // Register the schedules each loaded app/module declares in its
    // `Module Def.scheduler_events` (D3/SCHED-07). Idempotent by job name, and
    // a bad row is logged instead of preventing the scheduler from starting.
    register_module_scheduler_events(&scheduler, pool, jobs_cfg.default_queue.as_str()).await;

    scheduler.start().await?;
    let effective = scheduler.config();
    info!(
        "scheduler started (timezone {} [{}], tick {}s, max concurrent jobs {}, \
         history retention {}d)",
        scheduler.timezone(),
        effective.timezone,
        effective.tick_interval_secs,
        effective.max_concurrent_jobs,
        effective.history_retention_days
    );
    let scheduler = Arc::new(scheduler);
    Ok(scheduler)
}

/// Register the schedules a loaded module declares in `Module Def`.
///
/// `scheduler_events` is a JSON `kind -> [job]` map (the same vocabulary
/// Frappe uses); each job is registered on the given queue with its declared
/// cadence. The call is idempotent by job name, so an operator-tuned schedule
/// survives a restart, and one malformed row is logged and skipped — a typo in
/// one app must not keep the whole scheduler from starting (SCHED-07).
async fn register_module_scheduler_events(scheduler: &Scheduler, pool: &Db, default_queue: &str) {
    use ferris_core::Value;
    use ferris_document::{DocumentEngine, EngineContext, ListQuery};

    // Without the catalogue (a unit test that never booted the router) the
    // `Module Def` table has no schema to read; skip instead of creating a
    // metadata-less table.
    if ferris_schema::get_doctype("Module Def").is_none() {
        return;
    }

    let mut ctx = EngineContext::new("Module Def", "");
    // The host is the caller and only needs the declared JSON; neither
    // permissions nor lifecycle hooks apply to reading a declaration.
    ctx.allow_anonymous = true;
    ctx.suppress_events = true;
    let query = ListQuery {
        limit: Some(1000),
        ..Default::default()
    };
    let rows = match DocumentEngine::new(pool.clone()).list(&ctx, &query).await {
        Ok((rows, _total)) => rows,
        Err(error) => {
            warn!(error = %error, "could not read Module Def.scheduler_events");
            return;
        }
    };

    for row in rows {
        let module = row
            .get("module_name")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let Some(raw) = row.get("scheduler_events").and_then(Value::as_str) else {
            continue;
        };
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let events: std::collections::BTreeMap<String, Vec<String>> =
            match serde_json::from_str(raw) {
                Ok(events) => events,
                Err(error) => {
                    warn!(
                        module = %module,
                        error = %error,
                        "Module Def.scheduler_events is not a JSON map; skipped"
                    );
                    continue;
                }
            };
        match scheduler
            .register_scheduler_events(&events, default_queue)
            .await
        {
            Ok(ids) if ids.is_empty() => {}
            Ok(ids) => {
                info!(
                    module = %module,
                    schedules = ids.len(),
                    "scheduler_events registered"
                );
            }
            Err(error) => {
                warn!(
                    module = %module,
                    error = %error,
                    "Module Def.scheduler_events rejected"
                );
            }
        }
    }
}

#[cfg(all(test, feature = "scheduler"))]
mod scheduler_events_tests {
    use super::*;

    fn system_ctx(doctype: &'static str) -> ferris_document::EngineContext<'static> {
        let meta: &'static ferris_schema::DoctypeMeta = Box::leak(Box::new(
            ferris_schema::get_doctype(doctype).expect("registered"),
        ));
        let mut ctx = ferris_document::EngineContext::new(doctype, "");
        ctx.meta = Some(meta);
        let mut user = ferris_core::User::system_user("test@ferris.local".into(), "Test".into());
        user.roles.push("System Manager".into());
        ctx.user = Some(user);
        ctx
    }

    /// D3/SCHED-07: a `Module Def` row's `scheduler_events` becomes a schedule,
    /// with the cadence the row declared.
    #[tokio::test]
    async fn a_module_scheduler_events_row_registers_a_schedule() {
        ferris_schema::doctypes::register_doctypes();
        let db = ferris_db::memory_db().await.expect("in-memory db");
        ferris_document::DocumentEngine::new(db.clone())
            .create(
                &system_ctx("Module Def"),
                ferris_core::Value::from_json(serde_json::json!({
                    "module_name": "SchedulerEventsTest",
                    "scheduler_events": "{\"hourly\": [\"app.sweep\"]}",
                })),
            )
            .await
            .expect("module def row");

        let queue = Arc::new(JobQueue::new_db(QueueConfig::default(), db.clone()));
        queue.init().await.expect("queue init");
        let scheduler =
            Scheduler::with_config(queue, db.clone(), &ferris_core::SchedulerConfig::default())
                .await
                .expect("scheduler");
        register_module_scheduler_events(&scheduler, &db, "default").await;

        let schedules = scheduler.list_schedules().await.expect("schedules");
        let registered = schedules
            .iter()
            .find(|schedule| schedule.config.job_name == "app.sweep")
            .expect("the declared job is registered");
        assert_eq!(registered.config.cron_expression, "0 * * * *");
        scheduler.shutdown().await.expect("scheduler shutdown");
    }
}

/// An integrations sweep the scheduler seeds on fresh installs: `(job_name,
/// cron)`. Cadences trade urgency against cost — push delivery and reminders
/// run every minute (their due rows are minute-granular), auto email reports
/// hourly, auto repeat daily. `tokio-cron-scheduler` gets its leading seconds
/// field from `normalize_cron`.
#[cfg(all(feature = "scheduler", feature = "integrations"))]
const INTEGRATION_SWEEPS: [(&str, &str); 4] = [
    (
        ferris_integrations::runtime::jobs::JOB_AUTO_REPEAT,
        "0 0 * * *",
    ), // daily
    (
        ferris_integrations::runtime::jobs::JOB_REMINDER,
        "* * * * *",
    ), // every minute
    (
        ferris_integrations::runtime::jobs::JOB_AUTO_EMAIL_REPORT,
        "0 * * * *",
    ), // hourly
    (
        ferris_integrations::runtime::jobs::JOB_PUSH_DELIVERY,
        "* * * * *",
    ), // every minute
];

/// Ensure one enabled schedule exists for each integrations runtime job.
///
/// The seeds only fill gaps: any schedule already carrying the same `job_name`
/// (often an operator-tuned cadence) is left untouched, so this is safe on
/// every boot.
///
/// # Errors
///
/// Fails when the schedule list cannot be read or a seeded schedule cannot be
/// persisted.
#[cfg(all(feature = "scheduler", feature = "integrations"))]
async fn seed_integration_schedules(scheduler: &Scheduler, default_queue: &str) -> Result<()> {
    let existing = scheduler.list_schedules().await?;
    let existing_names: std::collections::HashSet<String> =
        existing.iter().map(|s| s.config.job_name.clone()).collect();

    for (job_name, cron) in INTEGRATION_SWEEPS {
        if existing_names.contains(job_name) {
            continue;
        }
        let schedule = Schedule::new(
            job_name.to_string(),
            ScheduleConfig {
                cron_expression: cron.to_string(),
                job_name: job_name.to_string(),
                queue: default_queue.to_string(),
                priority: JobPriority::Normal,
                ..ScheduleConfig::default()
            },
        );
        scheduler.add_schedule(schedule).await?;
        info!("seeded integration schedule {job_name} ({cron})");
    }
    Ok(())
}
