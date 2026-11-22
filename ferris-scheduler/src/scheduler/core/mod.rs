//! The `Scheduler` itself: persistence, the tick loop and the in-memory read
//! model.
//!
//! The in-memory map is the read model the API serves, so it must not drift
//! from the row it mirrors: every mutation persists first and only then updates
//! memory (SCH11), and a run re-reads the persisted row so counters and
//! `last_error` are visible without a restart (SCH3).
//!
//! # Multiple instances
//!
//! Every process loads the same `schedules` table, so without coordination each
//! one enqueues its own copy of every scheduled run. A database
//! `scheduler_lease` (see [`crate::leader`]) elects one leader: only the
//! leader's tick loop enqueues and only the leader performs the boot catch-up
//! (A12). Followers keep retrying the lease and take over when the holder stops
//! renewing it.
//!
//! # Timezone and DST
//!
//! The engine used to hand the cron expression to `tokio-cron-scheduler`, which
//! fixes the zone offset when the job is registered; a DST transition left the
//! job firing at the old offset (SCHED-02). The tick loop resolves every
//! deadline with [`CronExpression::next_occurrence_in`] in the configured
//! `chrono-tz` zone and stores the UTC instant, so the wall-clock hour survives
//! both transitions. The tick cadence is `tick_interval_secs`, which makes the
//! operator's setting effective (SCHED-01).

mod duties;
mod lease_loop;
mod tick_loop;

use crate::leader::LeaderLease;
use crate::persistence::ScheduleStore;
use crate::schedule::{
    CronExpression, Schedule, ScheduleConfig, ScheduleKind, parse_scheduler_events,
};
use chrono::Utc;
use chrono_tz::Tz;
use duties::LeaderDuties;
use ferris_core::{DocumentError, Error, JobError, Result, SchedulerConfig, ValidationError};
use ferris_jobs::{Job, JobQueue};
use lease_loop::{RuntimeSettings, lease_loop};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tick_loop::{TickContext, tick_loop};
use tokio::sync::{Mutex, Notify, RwLock, Semaphore};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use uuid::Uuid;

const MISFIRE_GRACE_SECS: i64 = 60;

/// The single lease every scheduler instance contends for. Named so a second,
/// independent scheduler fleet could use another name in the same database.
const LEASE_NAME: &str = "scheduler";

/// Build the config's Job and enqueue it, reusing the exact construction the
/// tick loop and the boot catch-up both need.
async fn build_and_enqueue(job_queue: &JobQueue, config: &ScheduleConfig) -> Result<String> {
    let mut job = Job::new(config.job_name.clone(), config.payload.clone());
    job.id = Uuid::new_v4().to_string();
    job.queue = config.queue.clone();
    job.priority = config.priority;
    job.timeout = config.timeout;
    job.max_retries = config.max_retries;
    job.tags = config.tags.clone();
    job.metadata = config.metadata.clone();
    job_queue.enqueue(job).await
}

/// Replace the in-memory schedule with the persisted row.
///
/// The tick loop used to mirror only `next_run`, so `list_schedules` /
/// `get_schedule` kept reporting `run_count = 0`, `failure_count = 0` and no
/// `last_error` after hundreds of runs while the row held the truth (SCH3).
/// Re-reading is one query per run and cannot disagree with the database.
async fn refresh_schedule(
    store: &ScheduleStore,
    schedules: &RwLock<HashMap<String, Schedule>>,
    schedule_id: &str,
) {
    match store.get(schedule_id).await {
        Ok(Some(stored)) => {
            // Take the lock only after the read: no guard is held across await.
            if let Some(slot) = schedules.write().await.get_mut(schedule_id) {
                *slot = stored;
            }
        }
        Ok(None) => {}
        Err(e) => error!("Failed to refresh schedule {schedule_id} after a run: {e}"),
    }
}

pub struct Scheduler {
    store: Arc<ScheduleStore>,
    job_queue: Arc<JobQueue>,
    schedules: Arc<RwLock<HashMap<String, Schedule>>>,
    /// Schedule ids currently eligible for the tick loop. Disabled schedules
    /// are persisted but never registered.
    registered: Arc<RwLock<HashSet<String>>>,
    lease: Arc<LeaderLease>,
    duties: Arc<LeaderDuties>,
    is_leader: Arc<AtomicBool>,
    /// Caps concurrent scheduled enqueues (`max_concurrent_jobs`), so a burst
    /// of due schedules does not open one task per schedule at once.
    enqueue_slots: Arc<Semaphore>,
    /// Wakes the tick loop when a schedule is added, enabled or disabled, so
    /// the change does not wait for the next interval.
    wake: Arc<Notify>,
    tick_task: Mutex<Option<JoinHandle<()>>>,
    lease_task: Mutex<Option<JoinHandle<()>>>,
    timezone: Tz,
    config: SchedulerConfig,
    settings: RuntimeSettings,
}

impl Scheduler {
    /// Build a scheduler with the default [`SchedulerConfig`].
    ///
    /// # Errors
    ///
    /// Fails when a table cannot be prepared.
    pub async fn new(job_queue: Arc<JobQueue>, db: ferris_db::Db) -> Result<Self> {
        Self::with_config(job_queue, db, &SchedulerConfig::default()).await
    }

    /// Build a scheduler from the process's scheduler configuration.
    ///
    /// Every operator knob is read here: `timezone` resolves cron deadlines,
    /// `tick_interval_secs` sizes the tick and the lease renewal/retry cadence,
    /// `max_concurrent_jobs` bounds concurrent scheduled enqueues and
    /// `history_retention_days` is the job-history purge window.
    ///
    /// # Errors
    ///
    /// Fails when `timezone` is not an IANA zone name or a table cannot be
    /// prepared. A misconfigured zone must fail at boot, not silently schedule
    /// in UTC.
    pub async fn with_config(
        job_queue: Arc<JobQueue>,
        db: ferris_db::Db,
        config: &SchedulerConfig,
    ) -> Result<Self> {
        let timezone: Tz = config.timezone.parse().map_err(|_| {
            Error::Validation(ValidationError::Invalid(format!(
                "scheduler.timezone {:?} is not a valid IANA timezone name (e.g. \"Europe/Madrid\")",
                config.timezone
            )))
        })?;

        let store = Arc::new(ScheduleStore::new(db.clone()));
        store.init().await?;

        let (settings, lease_ttl_secs) = RuntimeSettings::from_config(config);
        let lease = Arc::new(LeaderLease::new(
            db,
            LEASE_NAME,
            Uuid::new_v4().to_string(),
            lease_ttl_secs,
        ));
        lease.init().await?;

        let schedules = Arc::new(RwLock::new(HashMap::new()));
        let duties = Arc::new(LeaderDuties::new(
            store.clone(),
            job_queue.clone(),
            schedules.clone(),
            config.history_retention_days,
            lease.clone(),
            timezone,
        ));

        let scheduler = Self {
            store,
            job_queue,
            schedules,
            registered: Arc::new(RwLock::new(HashSet::new())),
            lease,
            duties,
            is_leader: Arc::new(AtomicBool::new(false)),
            enqueue_slots: Arc::new(Semaphore::new(config.max_concurrent_jobs.max(1))),
            wake: Arc::new(Notify::new()),
            tick_task: Mutex::new(None),
            lease_task: Mutex::new(None),
            timezone,
            config: config.clone(),
            settings,
        };

        // Load existing schedules. The missed-run catch-up is *not* done here:
        // it belongs to the leader, and a follower must not enqueue.
        scheduler.load_schedules().await?;

        Ok(scheduler)
    }

    /// Whether this instance currently holds the scheduler lease.
    #[must_use]
    pub fn is_leader(&self) -> bool {
        self.is_leader.load(Ordering::SeqCst)
    }

    /// The configuration this instance was built with.
    #[must_use]
    pub fn config(&self) -> &SchedulerConfig {
        &self.config
    }

    /// The zone cron expressions are interpreted in.
    #[must_use]
    pub fn timezone(&self) -> Tz {
        self.timezone
    }

    async fn load_schedules(&self) -> Result<()> {
        for schedule in self.store.list().await? {
            self.add_schedule_internal(schedule).await?;
        }

        info!("Loaded {} schedules", self.schedules.read().await.len());
        Ok(())
    }

    /// Persist, mirror and register a schedule, in that order.
    ///
    /// The row is written first; only then do memory and the tick loop see it,
    /// so a failed save never leaves a schedule that nothing will fire.
    async fn add_schedule_internal(&self, mut schedule: Schedule) -> Result<()> {
        // Disabled schedules are persisted but never registered.
        if schedule.enabled && schedule.next_run.is_none() {
            // A12: persist the first computed deadline for a schedule that has
            // never run. Only a persisted `next_run` lets the next boot tell a
            // missed execution apart from a schedule that simply hasn't fired.
            schedule.next_run = schedule.calculate_next_run_in(self.timezone, Utc::now());
            if schedule.next_run.is_none() {
                warn!(
                    "Schedule {} has cron {:?} with no next occurrence; it will not fire",
                    schedule.id, schedule.config.cron_expression
                );
            }
        }

        self.store.save(&schedule).await?;
        self.schedules
            .write()
            .await
            .insert(schedule.id.clone(), schedule.clone());

        if schedule.enabled {
            self.registered.write().await.insert(schedule.id.clone());
            self.wake.notify_one();
        }

        Ok(())
    }

    /// Add a new schedule and return **its** id.
    ///
    /// This used to return `HashMap::keys().last()`, an arbitrary key from the
    /// map, not the schedule that was just added (SCH1).
    pub async fn add_schedule(&self, schedule: Schedule) -> Result<String> {
        CronExpression::validate(&schedule.config.cron_expression)?;

        // Check for duplicate
        if self.schedules.read().await.contains_key(&schedule.id) {
            return Err(Error::Document(DocumentError::AlreadyExists {
                doctype: "Schedule".to_string(),
                name: schedule.id.clone(),
            }));
        }

        let schedule_id = schedule.id.clone();
        self.add_schedule_internal(schedule).await?;
        Ok(schedule_id)
    }

    /// Register a Frappe-style `scheduler_events` map (`kind -> [job]`).
    ///
    /// Idempotent by `job_name`: an existing schedule for the same job (often
    /// an operator-tuned cadence) wins, so calling this on every boot does not
    /// duplicate rows. `All` is due immediately (the next tick, i.e. startup)
    /// and then every tick via an every-second expression; the tick loop is
    /// what bounds its cadence.
    ///
    /// # Errors
    ///
    /// Fails on an invalid kind/job name, or when a schedule cannot be
    /// persisted.
    pub async fn register_scheduler_events(
        &self,
        events: &BTreeMap<String, Vec<String>>,
        queue: &str,
    ) -> Result<Vec<String>> {
        let parsed = parse_scheduler_events(events)?;
        let mut seen: HashSet<String> = self
            .schedules
            .read()
            .await
            .values()
            .map(|schedule| schedule.config.job_name.clone())
            .collect();

        let mut ids = Vec::new();
        for event in parsed {
            if !seen.insert(event.job_name.clone()) {
                continue;
            }

            let mut schedule = Schedule::new(
                event.job_name.clone(),
                ScheduleConfig {
                    cron_expression: event.kind.to_cron()?,
                    job_name: event.job_name,
                    queue: queue.to_string(),
                    ..ScheduleConfig::default()
                },
            );
            if event.kind == ScheduleKind::All {
                // `All` runs once as soon as the loop observes the schedule
                // (startup), then on every tick through the every-second cron.
                schedule.next_run = Some(Utc::now());
            }
            ids.push(self.add_schedule(schedule).await?);
        }
        Ok(ids)
    }

    /// Remove a schedule
    pub async fn remove_schedule(&self, schedule_id: &str) -> Result<()> {
        // The row is deleted first: it is the durable truth, and the tick loop
        // skips a schedule that is no longer in memory, so a tick that races
        // the removal is inert.
        self.store.delete(schedule_id).await?;
        self.schedules.write().await.remove(schedule_id);
        self.registered.write().await.remove(schedule_id);
        self.wake.notify_one();
        Ok(())
    }

    /// Enable a schedule
    pub async fn enable_schedule(&self, schedule_id: &str) -> Result<()> {
        let mut schedule = self
            .schedules
            .read()
            .await
            .get(schedule_id)
            .cloned()
            .ok_or_else(|| {
                Error::Job(JobError::NotFound(format!(
                    "Schedule {} not found",
                    schedule_id
                )))
            })?;

        schedule.enabled = true;
        schedule.updated_at = Utc::now();
        if schedule.next_run.is_none() {
            schedule.next_run = schedule.calculate_next_run_in(self.timezone, Utc::now());
        }

        self.store.save(&schedule).await?;
        self.schedules
            .write()
            .await
            .insert(schedule_id.to_string(), schedule);
        self.registered
            .write()
            .await
            .insert(schedule_id.to_string());
        self.wake.notify_one();
        Ok(())
    }

    /// Disable a schedule
    pub async fn disable_schedule(&self, schedule_id: &str) -> Result<()> {
        let mut schedule = self
            .schedules
            .read()
            .await
            .get(schedule_id)
            .cloned()
            .ok_or_else(|| {
                Error::Job(JobError::NotFound(format!(
                    "Schedule {} not found",
                    schedule_id
                )))
            })?;

        schedule.enabled = false;
        schedule.updated_at = Utc::now();

        self.store.save(&schedule).await?;
        self.schedules
            .write()
            .await
            .insert(schedule_id.to_string(), schedule);
        self.registered.write().await.remove(schedule_id);
        self.wake.notify_one();
        Ok(())
    }

    /// Trigger a schedule immediately
    pub async fn trigger_now(&self, schedule_id: &str) -> Result<()> {
        let schedules = self.schedules.read().await;

        if let Some(schedule) = schedules.get(schedule_id) {
            build_and_enqueue(&self.job_queue, &schedule.config).await?;
            Ok(())
        } else {
            Err(Error::Job(JobError::NotFound(format!(
                "Schedule {} not found",
                schedule_id
            ))))
        }
    }

    /// Get schedule by ID
    pub async fn get_schedule(&self, schedule_id: &str) -> Result<Option<Schedule>> {
        Ok(self.schedules.read().await.get(schedule_id).cloned())
    }

    /// List all schedules
    pub async fn list_schedules(&self) -> Result<Vec<Schedule>> {
        Ok(self.schedules.read().await.values().cloned().collect())
    }

    /// Start the tick loop and participate in the leadership election.
    ///
    /// The first lease attempt is synchronous, so a single-instance deployment
    /// has finished its A12 catch-up before `start` returns. Leadership is
    /// published *after* the catch-up, so the tick loop cannot enqueue a due
    /// schedule while the catch-up is still recovering the same run (SCHED-04).
    ///
    /// # Errors
    ///
    /// Currently infallible; kept `Result` for the callers that treat a boot
    /// failure as fatal. A failed lease attempt is not an error: the instance
    /// serves reads and retries in the background.
    pub async fn start(&self) -> Result<()> {
        match self.lease.try_acquire().await {
            Ok(true) => {
                if self.duties.on_became_leader().await {
                    self.is_leader.store(true, Ordering::SeqCst);
                    info!(
                        "Scheduler instance {} holds the leader lease",
                        self.lease.owner()
                    );
                } else {
                    warn!(
                        "Scheduler instance {} lost the lease during the boot catch-up",
                        self.lease.owner()
                    );
                }
            }
            Ok(false) => {
                info!(
                    "Scheduler instance {} is a follower; another instance owns the lease",
                    self.lease.owner()
                );
            }
            Err(e) => {
                // Not fatal: a follower can still serve the read model, and
                // the background loop retries. Leadership is required only to
                // enqueue.
                warn!("Failed to acquire the scheduler lease at start: {e}");
            }
        }

        self.spawn_tick_loop().await;
        self.spawn_lease_loop().await;

        info!(
            "Scheduler started with {} schedules",
            self.schedules.read().await.len()
        );
        Ok(())
    }

    /// Start the enqueueing tick loop, once.
    async fn spawn_tick_loop(&self) {
        let mut slot = self.tick_task.lock().await;
        if slot.is_some() {
            return;
        }

        let context = TickContext {
            store: self.store.clone(),
            job_queue: self.job_queue.clone(),
            schedules: self.schedules.clone(),
            is_leader: self.is_leader.clone(),
            slots: self.enqueue_slots.clone(),
            timezone: self.timezone,
        };
        let tick = self.settings.tick;
        let wake = self.wake.clone();
        *slot = Some(tokio::spawn(async move {
            tick_loop(context, tick, wake).await;
        }));
    }

    /// Start the lease renewal/acquisition loop, once.
    async fn spawn_lease_loop(&self) {
        let mut slot = self.lease_task.lock().await;
        if slot.is_some() {
            return;
        }

        let lease = self.lease.clone();
        let is_leader = self.is_leader.clone();
        let duties = self.duties.clone();
        let heartbeat = self.settings.heartbeat;
        let acquire_retry = self.settings.acquire_retry;
        *slot = Some(tokio::spawn(async move {
            lease_loop(lease, is_leader, duties, heartbeat, acquire_retry).await;
        }));
    }

    /// Shutdown the scheduler
    pub async fn shutdown(&self) -> Result<()> {
        let tick_task = self.tick_task.lock().await.take();
        if let Some(task) = tick_task {
            task.abort();
        }

        let lease_task = self.lease_task.lock().await.take();
        if let Some(task) = lease_task {
            task.abort();
        }

        // Give the lease up so a follower can take over without waiting for
        // the TTL; the local flag is cleared even if the delete fails.
        if self.is_leader.swap(false, Ordering::SeqCst)
            && let Err(e) = self.lease.release().await
        {
            error!("Failed to release the scheduler lease on shutdown: {e}");
        }

        info!("Scheduler shutdown");
        Ok(())
    }
}

#[cfg(test)]
mod tests;
