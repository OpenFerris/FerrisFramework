//! Job queue implementation with Redis + SQL persistence

use crate::retry::{ExponentialBackoff, RetryPolicy};
use crate::store::JobStore;
use crate::{Job, JobResult, JobStatus};
use chrono::{DateTime, Utc};
use ferris_core::error::JobError;
use ferris_core::{Error, Result};
use ferris_db::Db;
use redis::{AsyncCommands, Client as RedisClient};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

/// Queue configuration.
///
/// Only the knobs that change the queue's own behaviour live here. Priority,
/// concurrency, timeout and retry count are per-`Job` or per-`WorkerConfig`
/// decisions (`Job::with_priority`/`with_timeout`/`with_max_retries`,
/// `WorkerConfig::concurrency`). They used to be duplicated here, populated by
/// `ferris-server` and read by nothing (JOB11).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueConfig {
    pub name: String,
    pub retry_backoff_base_secs: u64,
    /// Queue a job moves to once it has exhausted its retries, so the source
    /// queue's backlog stays honest. A dead-lettered job is never claimed.
    pub dead_letter_queue: String,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            retry_backoff_base_secs: 60,
            dead_letter_queue: "dead_letter".to_string(),
        }
    }
}

/// The retry/backoff policy derived from a queue's configuration.
fn retry_policy_for(config: &QueueConfig) -> Arc<dyn RetryPolicy> {
    Arc::new(ExponentialBackoff {
        base_delay: Duration::from_secs(config.retry_backoff_base_secs),
        ..Default::default()
    })
}

/// Job queue with Redis + SQL persistence
pub struct JobQueue {
    config: QueueConfig,
    /// The Redis sorted-set half. `None` when the queue runs on the database
    /// alone (see [`JobQueue::new_db`]), which is the SQLite / edge case.
    redis: Option<RedisClient>,
    /// The durable half. Absent only between [`JobQueue::new`] and
    /// [`JobQueue::with_db`]; every operation that needs it fails loudly while
    /// it is missing (JOB4).
    store: Option<JobStore>,
    retry_policy: Arc<dyn RetryPolicy>,
}

impl JobQueue {
    pub async fn new(config: QueueConfig, redis: RedisClient) -> Result<Self> {
        let client = redis.clone();
        // Test the connection now so a bad URL fails at construction rather
        // than on the first enqueue.
        redis
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| JobError::Connection(format!("Redis connection failed: {e}")))?;

        let retry_policy = retry_policy_for(&config);

        Ok(Self {
            config,
            redis: Some(client),
            store: None,
            retry_policy,
        })
    }

    /// The durable half, or a loud error.
    ///
    /// A queue built with [`JobQueue::new`] has no store until [`with_db`] is
    /// called: its Redis sorted set is an enqueue accelerator, not a job
    /// source. Returning `Ok(None)`/`Ok(())` from the store-backed methods made
    /// such a queue accept jobs that nothing could ever claim or observe
    /// (JOB4), so they now fail loudly instead.
    ///
    /// [`with_db`]: JobQueue::with_db
    fn store(&self) -> Result<&JobStore> {
        self.store.as_ref().ok_or_else(|| {
            Error::Job(JobError::Connection(
                "job queue has no database backend; call JobQueue::with_db()".to_string(),
            ))
        })
    }

    /// The Redis sorted-set key of a queue.
    ///
    /// `queued` is `ZCARD` of this key, so its membership has to mean exactly
    /// "waiting to be claimed": `enqueue` inserts, and every path that moves a
    /// job out of the waiting state removes it again.
    fn zset_key(queue: &str) -> String {
        format!("queue:{queue}")
    }

    /// Insert `job_id` into its queue's sorted set, reporting a Redis failure.
    ///
    /// Enqueue keeps the failure visible: the caller asked for the job to be
    /// queued and the gauge it will be counted in was not updated.
    async fn zset_insert(&self, queue: &str, job_id: &str, score: i64) -> Result<()> {
        let Some(redis) = &self.redis else {
            return Ok(());
        };
        let mut conn = redis
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| JobError::Connection(format!("Redis connection failed: {e}")))?;
        let _: () = conn
            .zadd(Self::zset_key(queue), job_id, score as f64)
            .await
            .map_err(|e| JobError::Connection(format!("Redis zadd failed: {e}")))?;
        Ok(())
    }

    /// Remove `job_id` from its queue's sorted set.
    ///
    /// Best effort on purpose: every caller has already committed a durable
    /// state change (claim, completion, failure), so failing the operation
    /// afterwards would strand the job in a state the caller cannot retry.
    /// A failure is logged, not swallowed.
    async fn zset_remove(&self, queue: &str, job_id: &str) {
        let Some(redis) = &self.redis else {
            return;
        };
        let result = async {
            let mut conn = redis
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| JobError::Connection(format!("Redis connection failed: {e}")))?;
            let _: () = conn
                .zrem(Self::zset_key(queue), job_id)
                .await
                .map_err(|e| JobError::Connection(format!("Redis zrem failed: {e}")))?;
            Ok::<(), Error>(())
        };
        if let Err(e) = result.await {
            warn!("Could not remove job {job_id} from the Redis queue {queue}: {e}");
        }
    }

    /// Put a job back into its queue's sorted set after it became claimable
    /// again (a retry or an orphan recovery), best effort for the same reason
    /// as [`JobQueue::zset_remove`].
    async fn zset_requeue(&self, queue: &str, job_id: &str, score: i64) {
        if let Err(e) = self.zset_insert(queue, job_id, score).await {
            warn!("Could not return job {job_id} to the Redis queue {queue}: {e}");
        }
    }

    /// A queue backed by the database alone, with no Redis.
    ///
    /// `enqueue` persists to the `jobs` table and `pop_job` claims from it, so
    /// cron schedules and one-shot jobs run on SQLite (or any supported
    /// backend) without a Redis server. The caller must still [`init`] the
    /// store.
    ///
    /// [`init`]: JobQueue::init
    #[must_use]
    pub fn new_db(config: QueueConfig, db: Db) -> Self {
        let retry_policy = retry_policy_for(&config);
        Self {
            config,
            redis: None,
            store: Some(JobStore::new(db)),
            retry_policy,
        }
    }

    /// Back the queue with a database, so jobs survive a restart.
    pub fn with_db(mut self, db: Db) -> Self {
        self.store = Some(JobStore::new(db));
        self
    }

    /// Create the backing tables (no-op without a database).
    pub async fn init(&self) -> Result<()> {
        match &self.store {
            Some(store) => store.init().await,
            None => Ok(()),
        }
    }

    /// Record a worker heartbeat (upsert into `job_workers`).
    pub async fn heartbeat(
        &self,
        worker_id: &str,
        status: &str,
        queues: &[String],
        current_jobs: usize,
    ) -> Result<()> {
        self.store()?
            .heartbeat(worker_id, status, queues, current_jobs)
            .await
    }

    /// Recover jobs stuck in `running` (e.g. a worker died mid-job).
    ///
    /// A running job whose record has not been touched for `stale_after_secs`
    /// is put back into `retrying` so another worker can pick it up; one that
    /// has exhausted its retries moves to the configured dead-letter queue.
    /// The Redis sorted set is kept in step: a job put back on the queue is
    /// re-inserted, a job moved to the dead-letter queue leaves its source
    /// queue's set.
    pub async fn recover_orphaned_jobs(&self, stale_after_secs: i64) -> Result<usize> {
        let recovered = self
            .store()?
            .recover_orphaned(stale_after_secs, &self.config.dead_letter_queue)
            .await?;

        for recovered in &recovered {
            if recovered.job.status == JobStatus::Retrying {
                let due = recovered
                    .job
                    .scheduled_at
                    .map_or_else(|| Utc::now().timestamp(), |dt| dt.timestamp());
                self.zset_requeue(&recovered.job.queue, &recovered.job.id, due)
                    .await;
            } else {
                // Dead-lettered: it left `previous_queue` for good.
                self.zset_remove(&recovered.previous_queue, &recovered.job.id)
                    .await;
            }
        }

        Ok(recovered.len())
    }

    /// Enqueue a job (persisted when a pool is configured).
    pub async fn enqueue(&self, mut job: Job) -> Result<String> {
        job.status = JobStatus::Pending;
        job.updated_at = Utc::now();
        if let Some(store) = &self.store {
            store.persist(&job).await?;
        }
        // Add to Redis sorted set for the job's *own* queue so that stats and
        // scheduling reflect the queue the job was actually enqueued to. On a
        // database-only queue the durable row *is* the source of truth that
        // `pop_job` claims from, so there is no sorted set to update.
        if self.redis.is_some() {
            let score = job
                .scheduled_at
                .map_or_else(|| Utc::now().timestamp(), |dt| dt.timestamp());
            self.zset_insert(&job.queue, &job.id, score).await?;
        } else if self.store.is_none() {
            // Neither backend: the job would be lost the moment this returns.
            return Err(Error::Job(JobError::Connection(
                "job queue has neither a database nor a Redis backend".to_string(),
            )));
        }
        Ok(job.id)
    }

    /// Enqueue job at specific time
    pub async fn enqueue_at(&self, mut job: Job, run_at: DateTime<Utc>) -> Result<String> {
        job.scheduled_at = Some(run_at);
        job.status = JobStatus::Pending;
        self.enqueue(job).await
    }

    /// Enqueue multiple jobs
    pub async fn enqueue_batch(&self, jobs: Vec<Job>) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for job in jobs {
            let id = self.enqueue(job).await?;
            ids.push(id);
        }
        Ok(ids)
    }

    /// Get job by ID
    ///
    /// # Errors
    ///
    /// Fails when the queue has no database backend or the row cannot be read.
    pub async fn get_job(&self, job_id: &str) -> Result<Option<Job>> {
        self.store()?.get(job_id).await
    }

    /// Atomically claim the next due job for a specific queue.
    ///
    /// The Redis sorted set is the `queued` gauge (`ZCARD`), so a claimed job
    /// leaves it here. `enqueue` used to `ZADD` and nothing ever `ZREM`ed, so
    /// every job ever enqueued kept counting as waiting.
    ///
    /// # Errors
    ///
    /// Fails when the queue has no database backend or the claim cannot run.
    pub async fn pop_job(&self, worker_id: &str, queue_name: Option<&str>) -> Result<Option<Job>> {
        let claimed = self
            .store()?
            .claim(
                worker_id,
                queue_name.unwrap_or(&self.config.name),
                Utc::now(),
            )
            .await?;

        if let Some(job) = &claimed {
            self.zset_remove(&job.queue, &job.id).await;
        }
        Ok(claimed)
    }

    /// Complete job successfully
    pub async fn complete_job(&self, job_id: &str, result: JobResult) -> Result<()> {
        let job = self
            .update_job(job_id, |job| {
                job.status = JobStatus::Success;
                job.completed_at = Some(Utc::now());
                job.result = Some(result);
            })
            .await?;
        self.zset_remove(&job.queue, job_id).await;
        Ok(())
    }

    /// Fail a job, scheduling a retry with backoff when retries remain.
    ///
    /// # Errors
    ///
    /// `JobError::NotFound` when the id no longer exists, and a concurrent
    /// status change is reported rather than overwritten (JOB5/JOB13).
    pub async fn fail_job(&self, job_id: &str, error: String) -> Result<()> {
        let store = self.store()?;
        let Some(mut job) = store.get(job_id).await? else {
            return Err(Error::Job(JobError::NotFound(job_id.to_string())));
        };
        let expected = job.status;
        // The queue the job is leaving; `job.queue` itself may change to the
        // dead-letter queue below.
        let previous_queue = job.queue.clone();

        let now = Utc::now();
        job.retries += 1;
        job.updated_at = now;
        // Mark Failed *before* checking `can_retry` (which requires Failed/Retrying).
        job.status = JobStatus::Failed;
        let duration = job
            .started_at
            .map(|s| (now - s).num_milliseconds().max(0) as u64)
            .unwrap_or(0);
        job.result = Some(JobResult {
            success: false,
            output: None,
            error: Some(error),
            started_at: job.started_at.unwrap_or(now),
            completed_at: now,
            duration_ms: duration,
        });

        if job.can_retry() {
            job.status = JobStatus::Retrying;
            // Use the configured retry policy (default: exponential backoff
            // with jitter) rather than a hardcoded delay.
            let delay = self.retry_policy.next_delay(job.retries.saturating_sub(1));
            job.scheduled_at = Some(now + chrono::Duration::seconds(delay.as_secs() as i64));
        } else {
            job.status = JobStatus::DeadLetter;
            job.completed_at = Some(now);
            job.scheduled_at = None;
            // Move it out of the live queue into the configured dead-letter
            // queue (JOB11). The status alone was not enough: the job stayed
            // in its source queue and inflated that queue's history.
            job.queue = self.config.dead_letter_queue.clone();
        }

        if !store.update_guarded(&job, expected).await? {
            return Err(Error::Job(JobError::Failed(format!(
                "job {job_id} changed concurrently while failing (was {expected:?})"
            ))));
        }

        if job.status == JobStatus::Retrying {
            // Back in the waiting set with its backoff deadline as the score,
            // so `queued` counts it exactly like the SQL count does.
            let due = job
                .scheduled_at
                .map_or_else(|| Utc::now().timestamp(), |dt| dt.timestamp());
            self.zset_requeue(&job.queue, job_id, due).await;
        } else {
            self.zset_remove(&previous_queue, job_id).await;
        }
        Ok(())
    }

    /// Cancel a job
    pub async fn cancel_job(&self, job_id: &str) -> Result<()> {
        let job = self
            .update_job(job_id, |job| {
                job.status = JobStatus::Cancelled;
                job.completed_at = Some(Utc::now());
            })
            .await?;
        self.zset_remove(&job.queue, job_id).await;
        Ok(())
    }

    /// Retry a job immediately
    pub async fn retry_job(&self, job_id: &str) -> Result<()> {
        let job = self
            .update_job(job_id, |job| {
                job.status = JobStatus::Retrying;
                job.scheduled_at = Some(Utc::now());
            })
            .await?;
        let due = job
            .scheduled_at
            .map_or_else(|| Utc::now().timestamp(), |dt| dt.timestamp());
        self.zset_requeue(&job.queue, job_id, due).await;
        Ok(())
    }

    /// List jobs with filters
    pub async fn list_jobs(
        &self,
        status: Option<JobStatus>,
        queue: Option<String>,
        limit: usize,
    ) -> Result<Vec<Job>> {
        self.store()?
            .list(
                &queue.unwrap_or_else(|| self.config.name.clone()),
                status,
                limit,
            )
            .await
    }

    /// Every worker that has heartbeated for this queue's store, for the
    /// server's `RQ Worker` observability mirror (SRV7).
    ///
    /// # Errors
    ///
    /// Fails when the worker table cannot be read.
    // `Error` is the crate-wide error type; boxing it only for this read would
    // leak into the caller's signature for no benefit.
    #[allow(clippy::result_large_err)]
    pub async fn list_workers(&self) -> Result<Vec<crate::store::JobWorker>> {
        self.store()?.list_workers().await
    }

    /// Load, mutate and write a job back under a status guard.
    ///
    /// The guard is what turns the old lost-update window into a typed error:
    /// if the row's status changed between the read and the write, the caller
    /// gets `JobError::Failed` instead of silently discarding the other
    /// writer's change (JOB5). The mutated job is returned so the caller can
    /// update the Redis sorted set of the queue it actually landed in.
    async fn update_job(&self, job_id: &str, mutate: impl FnOnce(&mut Job)) -> Result<Job> {
        let store = self.store()?;
        let Some(mut job) = store.get(job_id).await? else {
            return Err(Error::Job(JobError::NotFound(job_id.to_string())));
        };
        let expected = job.status;
        mutate(&mut job);
        job.updated_at = Utc::now();
        if !store.update_guarded(&job, expected).await? {
            return Err(Error::Job(JobError::Failed(format!(
                "job {job_id} changed concurrently (was {expected:?})"
            ))));
        }
        Ok(job)
    }

    /// Delete terminal jobs that finished before `before`.
    ///
    /// Used by the scheduler's `history_retention_days`. Only terminal rows are
    /// touched, so a pending or running job is never pruned; the dependency
    /// edges of the pruned rows go with them.
    ///
    /// # Errors
    ///
    /// Fails when the queue has no database backend or the delete cannot run.
    pub async fn purge_terminal_jobs(&self, before: DateTime<Utc>) -> Result<usize> {
        self.store()?.purge_terminal_before(before).await
    }

    /// Get queue stats for this queue instance.
    pub async fn get_stats(&self) -> Result<QueueStats> {
        self.get_queue_stats(&self.config.name).await
    }

    /// Get queue stats for a specific queue (may be a different queue than the
    /// one this instance is configured for).
    ///
    /// `queued` reads the Redis sorted set when there is one, which is why
    /// claim/complete/fail/recovery all maintain it: `ZCARD` only means
    /// "waiting" if the members that stopped waiting are removed. On a
    /// database-only queue the count comes from the `jobs` rows.
    pub async fn get_queue_stats(&self, queue: &str) -> Result<QueueStats> {
        // With Redis the sorted set is the count of enqueued-but-not-claimed
        // jobs; on a database-only queue the `jobs` rows play that role. A
        // `zcard` failure is reported, not reported as "0 queued" (JOB8).
        let queued: usize = if let Some(redis) = &self.redis {
            let mut conn = redis
                .get_multiplexed_async_connection()
                .await
                .map_err(|e| JobError::Connection(format!("Redis connection failed: {e}")))?;
            let queued: u64 = conn
                .zcard(Self::zset_key(queue))
                .await
                .map_err(|e| JobError::Connection(format!("Redis zcard failed: {e}")))?;
            usize::try_from(queued).unwrap_or(usize::MAX)
        } else {
            usize::try_from(self.store()?.queued_count(queue).await?).unwrap_or(usize::MAX)
        };

        // `processing` is the running count of this queue, read from the same
        // durable rows the claim writes (JOB8).
        let processing =
            usize::try_from(self.store()?.running_count(queue).await?).unwrap_or(usize::MAX);

        let mut stats = QueueStats {
            queue_name: queue.to_string(),
            queued,
            processing,
            ..Default::default()
        };

        for (status, count) in self.store()?.terminal_counts(queue).await? {
            let count = count as usize;
            match status.as_str() {
                "success" => stats.completed = count,
                "failed" => stats.failed = count,
                "dead_letter" => stats.dead_letter = count,
                _ => {}
            }
        }

        Ok(stats)
    }
}

/// Queue statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueueStats {
    pub queue_name: String,
    pub queued: usize,
    pub processing: usize,
    pub completed: usize,
    pub failed: usize,
    pub dead_letter: usize,
}

#[cfg(test)]
mod tests;
