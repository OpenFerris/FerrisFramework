//! Worker pool for executing jobs.
//!
//! The worker owns four tasks: the polling intake ([`processing`]), the
//! heartbeat, the orphan sweep and the per-job supervisor. The intake lives in
//! `processing.rs` so the public surface (`Worker`, `WorkerConfig`,
//! `WorkerPool`) stays here and the loop can be read on its own.

mod processing;

use crate::{JobHandler, JobQueue};
use ferris_core::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::{Duration, interval};
use tracing::{debug, info, warn};

/// How often a worker records its liveness. Orphan recovery treats a job as
/// abandoned once its owner has missed three of these.
const HEARTBEAT_INTERVAL_SECS: u64 = 10;

/// A `running` job whose worker has not heartbeated for three intervals is an
/// orphan: the worker died and another one must reclaim the job (JOB-01).
/// Kept in sync with [`HEARTBEAT_INTERVAL_SECS`] by a unit test.
const ORPHAN_AFTER_SECS: i64 = 30;

/// Worker configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub worker_id: String,
    pub queues: Vec<String>,
    pub concurrency: usize,
    pub poll_interval_ms: u64,
    pub shutdown_timeout_secs: u64,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_id: uuid::Uuid::new_v4().to_string(),
            queues: vec!["default".to_string()],
            concurrency: 10,
            poll_interval_ms: 1000,
            shutdown_timeout_secs: 30,
        }
    }
}

/// Worker information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerInfo {
    pub worker_id: String,
    pub status: WorkerStatus,
    pub queues: Vec<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
    pub jobs_processed: u64,
    pub jobs_failed: u64,
    pub current_jobs: usize,
    pub max_concurrency: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Starting,
    Running,
    Stopping,
    Stopped,
}

/// Generic worker that can execute any job type
pub struct Worker<H: JobHandler + 'static> {
    config: WorkerConfig,
    handler: Arc<H>,
    queue: Arc<JobQueue>,
    semaphore: Arc<tokio::sync::Semaphore>,
    status: Arc<parking_lot::RwLock<WorkerStatus>>,
    stats: Arc<parking_lot::RwLock<WorkerStats>>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    started_at: chrono::DateTime<chrono::Utc>,
    last_heartbeat: Arc<parking_lot::RwLock<chrono::DateTime<chrono::Utc>>>,
}

#[derive(Debug, Default, Clone)]
struct WorkerStats {
    jobs_processed: u64,
    jobs_failed: u64,
    current_jobs: usize,
}

impl WorkerStats {
    /// Release one job slot without underflowing if the increment/decrement
    /// pairing ever drifts (debug builds panic on `usize - 1` at zero).
    fn finish_job(&mut self) {
        self.current_jobs = self.current_jobs.saturating_sub(1);
    }
}

/// Releases a job's slot when its supervisor ends.
///
/// A handler panic unwinds through the *run* task; the supervisor observes the
/// `JoinError` and the slot is released by `Drop` regardless. Before this, a
/// panic skipped `finish_job`, so `current_jobs` lied and `shutdown` waited the
/// full timeout for a job that no longer existed (JOB-02).
struct JobSlot {
    stats: Arc<parking_lot::RwLock<WorkerStats>>,
}

impl JobSlot {
    const fn new(stats: Arc<parking_lot::RwLock<WorkerStats>>) -> Self {
        Self { stats }
    }
}

impl Drop for JobSlot {
    fn drop(&mut self) {
        self.stats.write().finish_job();
    }
}

impl<H: JobHandler + 'static> Worker<H> {
    pub fn new(config: WorkerConfig, handler: H, queue: Arc<JobQueue>) -> Self {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        // Honour the configured concurrency instead of a hardcoded value.
        let concurrency = config.concurrency.max(1);
        let now = chrono::Utc::now();

        Self {
            config,
            handler: Arc::new(handler),
            queue,
            semaphore: Arc::new(Semaphore::new(concurrency)),
            status: Arc::new(parking_lot::RwLock::new(WorkerStatus::Starting)),
            stats: Arc::new(parking_lot::RwLock::new(WorkerStats::default())),
            shutdown_tx,
            shutdown_rx,
            started_at: now,
            last_heartbeat: Arc::new(parking_lot::RwLock::new(now)),
        }
    }

    /// Start the worker, stopping only on [`Worker::shutdown`].
    ///
    /// # Errors
    ///
    /// Currently infallible; kept fallible so a future startup check does not
    /// change the call sites.
    pub async fn start(&mut self) -> Result<()> {
        self.start_with(None).await
    }

    /// Start the worker, also stopping when `external` flips to `true`.
    ///
    /// [`WorkerPool`] passes its own receiver here. Before this, `add_worker`
    /// subscribed to the pool channel and then dropped the receiver, while the
    /// spawned worker watched a private channel that nothing could reach — so
    /// `pool.shutdown()` awaited a task parked forever and process shutdown
    /// hung (JOB1).
    ///
    /// # Errors
    ///
    /// Currently infallible; see [`Worker::start`].
    pub async fn start_with(
        &mut self,
        mut external: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Result<()> {
        *self.status.write() = WorkerStatus::Running;
        info!("Worker {} started", self.config.worker_id);

        // Start heartbeat
        let heartbeat_handle = self.start_heartbeat();

        // Start job processing loop
        let process_handle = self.start_processing();

        // Reclaim jobs left `running` by a worker that died (JOB-01).
        let recovery_handle = self.start_recovery();

        // Wait for shutdown signal, from the worker itself or from the pool
        // that owns it.
        match external.as_mut() {
            Some(pool) => {
                tokio::select! {
                    _ = self.shutdown_rx.changed() => {}
                    _ = pool.changed() => {}
                }
            }
            None => {
                self.shutdown_rx.changed().await.ok();
            }
        }

        *self.status.write() = WorkerStatus::Stopping;
        info!("Worker {} stopping...", self.config.worker_id);

        // Wait for active jobs to complete (with timeout)
        self.wait_for_completion().await;

        *self.status.write() = WorkerStatus::Stopped;
        info!("Worker {} stopped", self.config.worker_id);

        heartbeat_handle.abort();
        recovery_handle.abort();
        process_handle.abort();

        Ok(())
    }

    /// Reclaim jobs stuck in `running` whose owner stopped heartbeating.
    ///
    /// `recover_orphaned_jobs` was only reached from tests, so a worker that
    /// crashed mid-job left the row `running` forever (JOB-01). Every worker
    /// now sweeps at startup and once per three heartbeat intervals; the store
    /// guards each row update, so concurrent sweepers cannot reclaim the same
    /// job twice.
    fn start_recovery(&self) -> tokio::task::JoinHandle<()> {
        let queue = self.queue.clone();
        let mut shutdown_rx = self.shutdown_rx.clone();

        tokio::spawn(async move {
            // `interval`'s first tick completes immediately, so the sweep also
            // runs at startup.
            let mut interval = interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS * 3));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if *shutdown_rx.borrow() { break; }
                        match queue.recover_orphaned_jobs(ORPHAN_AFTER_SECS).await {
                            Ok(0) => {}
                            Ok(recovered) => info!("Recovered {recovered} orphaned job(s)"),
                            Err(e) => warn!("Orphaned-job recovery failed: {e}"),
                        }
                    }
                    _ = shutdown_rx.changed() => break,
                }
            }
        })
    }

    /// One sweep with an explicit staleness window.
    ///
    /// The periodic task uses [`ORPHAN_AFTER_SECS`]; tests pass a negative
    /// window because a claim stamped in the current second is not yet older
    /// than "now".
    #[cfg(test)]
    // Same crate-wide `Result` as every queue operation; boxing only this one
    // would change the test's signature for no benefit.
    #[allow(clippy::result_large_err)]
    async fn recover_orphans(&self, stale_after_secs: i64) -> Result<usize> {
        self.queue.recover_orphaned_jobs(stale_after_secs).await
    }

    /// Start heartbeat task: persist the worker's liveness so that jobs owned
    /// by a dead worker can be recovered.
    fn start_heartbeat(&self) -> tokio::task::JoinHandle<()> {
        let worker_id = self.config.worker_id.clone();
        let queues = self.config.queues.clone();
        let status = self.status.clone();
        let stats = self.stats.clone();
        let queue = self.queue.clone();
        let last_heartbeat = self.last_heartbeat.clone();
        let mut shutdown_rx = self.shutdown_rx.clone();

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if *shutdown_rx.borrow() { break; }
                        let status_str = format!("{:?}", *status.read()).to_lowercase();
                        let current = stats.read().current_jobs;
                        if let Err(e) = queue.heartbeat(&worker_id, &status_str, &queues, current).await {
                            debug!("Worker {} heartbeat failed: {}", worker_id, e);
                        }
                        *last_heartbeat.write() = chrono::Utc::now();
                    }
                    _ = shutdown_rx.changed() => {
                        break;
                    }
                }
            }
        })
    }

    /// Wait for active jobs to complete
    async fn wait_for_completion(&self) {
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(self.config.shutdown_timeout_secs);

        while self.stats.read().current_jobs > 0 {
            if start.elapsed() > timeout {
                warn!(
                    "Shutdown timeout reached, {} jobs still running",
                    self.stats.read().current_jobs
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Request shutdown
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Get worker info
    pub fn get_info(&self) -> WorkerInfo {
        let stats = self.stats.read();
        WorkerInfo {
            worker_id: self.config.worker_id.clone(),
            status: *self.status.read(),
            queues: self.config.queues.clone(),
            started_at: self.started_at,
            last_heartbeat: *self.last_heartbeat.read(),
            jobs_processed: stats.jobs_processed,
            jobs_failed: stats.jobs_failed,
            current_jobs: stats.current_jobs,
            max_concurrency: self.config.concurrency,
        }
    }
}

/// Worker pool for managing multiple workers
pub struct WorkerPool {
    workers: Vec<tokio::task::JoinHandle<()>>,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl WorkerPool {
    pub fn new() -> Self {
        let (shutdown_tx, _) = tokio::sync::watch::channel(false);
        Self {
            workers: Vec::new(),
            shutdown_tx,
        }
    }

    /// Spawn `worker`, handing it the pool's shutdown signal.
    pub fn add_worker<H: JobHandler + 'static>(&mut self, mut worker: Worker<H>) {
        let shutdown_rx = self.shutdown_tx.subscribe();
        let handle = tokio::spawn(async move {
            let _ = worker.start_with(Some(shutdown_rx)).await;
        });
        self.workers.push(handle);
    }

    /// Stop every worker and wait for it to finish draining.
    pub async fn shutdown(&mut self) {
        let _ = self.shutdown_tx.send(true);
        for handle in self.workers.drain(..) {
            let _ = handle.await;
        }
    }
}

impl Default for WorkerPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
