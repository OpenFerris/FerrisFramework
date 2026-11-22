//! Durable job rows: creation, claim, guarded update, recovery and counts.

use chrono::{DateTime, Utc};
use ferris_core::{Result, Value};
use ferris_db::{
    Db, Stmt,
    sql::{UpsertBuilder, UpsertSet},
};

use super::columns::{MUTABLE_COLUMNS, job_columns, json, ts};
use super::schema::{
    TAB_JOB_DEPENDENCIES, TAB_JOB_WORKERS, TAB_JOBS, job_dependencies_table, job_workers_table,
    jobs_table,
};
use crate::job::{Job, JobStatus};

/// A job that `recover_orphaned` reclaimed, with the queue it came from.
///
/// `job.queue` is the queue **after** recovery: a job whose retries are
/// exhausted has already moved to the dead-letter queue, so the caller needs
/// the previous queue to remove it from that queue's Redis sorted set.
pub struct RecoveredJob {
    pub previous_queue: String,
    pub job: Job,
}

/// The dependency ids worth indexing, deduplicated and without blanks.
///
/// `depends_on` comes from user metadata (`#[job]`/WASM), so a repeated or
/// empty id must not turn into a duplicate-key error on the edge table.
fn unique_dependencies(depends_on: &[String]) -> Vec<&str> {
    let mut seen = std::collections::HashSet::new();
    depends_on
        .iter()
        .map(String::as_str)
        .filter(|id| !id.is_empty() && seen.insert(*id))
        .collect()
}

/// The SQL half of the job queue.
#[derive(Clone)]
pub struct JobStore {
    pub(super) db: Db,
}

impl JobStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Create the backing tables and the indexes the claim and listing queries
    /// rely on.
    pub async fn init(&self) -> Result<()> {
        self.db.ensure_table(TAB_JOBS, &jobs_table()).await?;
        // `claim` reads the next due job for a queue by
        // (queue, status, priority, created_at).
        self.db
            .ensure_index(
                "idx_jobs_claim",
                TAB_JOBS,
                &["queue", "status", "priority", "created_at"],
                false,
            )
            .await?;
        // `list` reads one queue newest-first.
        self.db
            .ensure_index(
                "idx_jobs_queue_created",
                TAB_JOBS,
                &["queue", "created_at"],
                false,
            )
            .await?;
        // `recover_orphaned` scans by status and age.
        self.db
            .ensure_index(
                "idx_jobs_status_updated",
                TAB_JOBS,
                &["status", "updated_at"],
                false,
            )
            .await?;
        self.db
            .ensure_table(TAB_JOB_WORKERS, &job_workers_table())
            .await?;
        // Dependency edges are read by the claim's `NOT EXISTS` subquery,
        // keyed by the dependent job.
        self.db
            .ensure_table(TAB_JOB_DEPENDENCIES, &job_dependencies_table())
            .await
    }

    /// Write a job out whole, replacing any row with the same id.
    ///
    /// `payload` holds the serialised [`Job`], which is what every read path
    /// decodes; the columns beside it exist so the queue can be claimed and
    /// counted in SQL without decoding every row.
    ///
    /// The dependency edges are rewritten in the same transaction, so the
    /// claim never sees a row whose `depends_on` column and dependency index
    /// disagree.
    ///
    /// # Errors
    ///
    /// Fails when the job or one of its dependencies cannot be encoded, or the
    /// write cannot run.
    pub async fn persist(&self, job: &Job) -> Result<()> {
        let columns = job_columns(job)?;
        let mut builder = UpsertBuilder::new(self.db.dialect(), TAB_JOBS)
            .values(columns.clone())
            .conflict_on(["id"]);
        for (name, value) in &columns {
            if MUTABLE_COLUMNS.contains(name) {
                builder = builder.on_update(*name, UpsertSet::Value(value.clone()));
            }
        }

        let mut tx = self.db.begin().await?;
        tx.upsert(builder.build()?).await?;
        tx.execute(&Stmt::new(
            format!("DELETE FROM {TAB_JOB_DEPENDENCIES} WHERE job_id = ?"),
            vec![Value::from(job.id.as_str())],
        ))
        .await?;
        for dependency in unique_dependencies(&job.depends_on) {
            tx.execute(&Stmt::new(
                format!("INSERT INTO {TAB_JOB_DEPENDENCIES} (job_id, depends_on_id) VALUES (?, ?)"),
                vec![Value::from(job.id.as_str()), Value::from(dependency)],
            ))
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Write every mutable column of `job`, but only while the row still holds
    /// `expected_status`.
    ///
    /// `complete_job`/`fail_job`/`cancel_job` used to decode the row, mutate
    /// their copy and write the whole `Job` back, so two writers (the worker
    /// finishing, an operator cancelling, orphan recovery) silently discarded
    /// each other's change. The guard makes the loser of that race visible
    /// (JOB5); `Ok(false)` means "someone else got there first".
    ///
    /// # Errors
    ///
    /// Fails when the row cannot be encoded or the `UPDATE` cannot run.
    pub async fn update_guarded(&self, job: &Job, expected_status: JobStatus) -> Result<bool> {
        let columns = job_columns(job)?;
        let mut assignments = Vec::with_capacity(columns.len());
        let mut params = Vec::with_capacity(columns.len());
        for (name, value) in columns {
            if name == "id" {
                continue;
            }
            assignments.push(format!("{name} = ?"));
            params.push(value);
        }
        params.push(Value::from(job.id.as_str()));
        params.push(Value::from(expected_status.as_db_str()));

        let affected = self
            .db
            .execute(&Stmt::new(
                format!(
                    "UPDATE {TAB_JOBS} SET {} WHERE id = ? AND status = ?",
                    assignments.join(", ")
                ),
                params,
            ))
            .await?;
        Ok(!affected.is_empty())
    }

    pub async fn get(&self, job_id: &str) -> Result<Option<Job>> {
        let row = self
            .db
            .fetch_optional(&Stmt::new(
                format!("SELECT payload FROM {TAB_JOBS} WHERE id = ?"),
                vec![Value::from(job_id)],
            ))
            .await?;

        row.map(|row| Ok(serde_json::from_str(&row.str("payload")?)?))
            .transpose()
    }

    /// Claim the next due job for `queue`, handing it to `worker_id`.
    ///
    /// The candidate read and the guarded update share a transaction, but the
    /// update is what actually decides: two workers can read the same
    /// candidate and only the one whose `UPDATE` still matches a claimable
    /// status walks away with the job.
    ///
    /// A job naming dependencies is only eligible once all of them are
    /// `success`; `depends_on` used to be persisted and then ignored here, so
    /// a dependent job ran immediately. A dependency whose row no longer
    /// exists (pruned history, a typo) is treated as satisfied rather than
    /// blocking the queue forever; a dependency that ended in `dead_letter`
    /// or `cancelled` keeps the dependent blocked, which is the point of the
    /// dependency.
    pub async fn claim(
        &self,
        worker_id: &str,
        queue: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<Job>> {
        let mut tx = self.db.begin().await?;

        let row = tx
            .fetch_optional(&Stmt::new(
                format!(
                    "SELECT j.id, j.payload FROM {TAB_JOBS} j \
                     WHERE j.queue = ? AND j.status IN ('pending', 'retrying') \
                       AND (j.scheduled_at IS NULL OR j.scheduled_at <= ?) \
                       AND NOT EXISTS ( \
                           SELECT 1 FROM {TAB_JOB_DEPENDENCIES} d \
                           JOIN {TAB_JOBS} dep ON dep.id = d.depends_on_id \
                           WHERE d.job_id = j.id AND dep.status <> 'success' \
                       ) \
                     ORDER BY j.priority DESC, j.created_at ASC \
                     LIMIT 1"
                ),
                vec![Value::from(queue), Value::from(now.timestamp())],
            ))
            .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let mut job: Job = serde_json::from_str(&row.str("payload")?)?;

        // The payload is the document every reader decodes, so the claim has to
        // move it together with the columns. Updating only the columns left
        // `get` reporting the pre-claim status and made the guarded write in
        // `fail_job`/`update_job` see a stale expected status (JOB5).
        job.status = JobStatus::Running;
        job.worker_id = Some(worker_id.to_string());
        job.started_at = Some(now);
        job.updated_at = now;
        let payload = json(&job)?;

        let updated = tx
            .execute(&Stmt::new(
                format!(
                    "UPDATE {TAB_JOBS} SET status = 'running', worker_id = ?, \
                     started_at = ?, updated_at = ?, payload = ? \
                     WHERE id = ? AND status IN ('pending', 'retrying')"
                ),
                vec![
                    Value::from(worker_id),
                    Value::from(now.timestamp()),
                    Value::from(now.timestamp()),
                    payload,
                    Value::from(job.id.as_str()),
                ],
            ))
            .await?;

        if updated.is_empty() {
            return Ok(None);
        }
        tx.commit().await?;

        Ok(Some(job))
    }

    /// Put `running` jobs whose worker has gone quiet back into `retrying`, so
    /// another worker can pick them up.
    ///
    /// Orphanhood is decided by the worker's heartbeat: a running
    /// job is only reclaimable when the worker that claimed it is no longer
    /// heartbeating — `updated_at` alone is frozen at claim time, so judging by
    /// job age alone used to re-queue a healthy long-running job while its
    /// worker was still processing it, running it twice. The claim has to be
    /// old *and* the heartbeat stale before the job is touched.
    ///
    /// Each recovery consumes one retry. Once the job has exhausted
    /// `max_retries` it is moved to `dead_letter` instead of being requeued,
    /// so a poisoned job cannot loop through the queue forever.
    ///
    /// The update is guarded per row (`status = 'running'`), so two concurrent
    /// recoverers cannot both reclaim the same job. The status/retries and the
    /// `payload` document are derived from **one** decoded snapshot and written
    /// in **one** statement: the old code updated the columns first and then
    /// mirrored them from a second read with fresh `Utc::now()` values, so a
    /// crash or a concurrent claim could leave the two disagreeing and the
    /// retry cap could be evaluated against two different sources (JOB6).
    ///
    /// A job that has exhausted its retries is moved to `dead_letter_queue`,
    /// not left in the source queue (JOB11).
    ///
    /// Returns the reclaimed jobs together with the queue each came from, so
    /// the caller can maintain the Redis sorted sets: a retried job is waiting
    /// again, a dead-lettered one left its source queue.
    pub async fn recover_orphaned(
        &self,
        stale_after_secs: i64,
        dead_letter_queue: &str,
    ) -> Result<Vec<RecoveredJob>> {
        let now = Utc::now();
        let cutoff = now.timestamp() - stale_after_secs;

        // A worker row exists only while the worker keeps heartbeating. A job
        // with no worker row (either no `worker_id` or a worker that never
        // heartbeated) still needs its own claim to be stale, so a job claimed
        // moments ago is never reclaimed in the heartbeat's first interval.
        let stale = self
            .db
            .fetch_all(&Stmt::new(
                format!(
                    "SELECT j.id, j.payload FROM {TAB_JOBS} j \
                     LEFT JOIN {TAB_JOB_WORKERS} w ON j.worker_id = w.worker_id \
                     WHERE j.status = 'running' \
                       AND (j.updated_at IS NULL OR j.updated_at < ?) \
                       AND (w.worker_id IS NULL OR w.last_heartbeat IS NULL \
                            OR w.last_heartbeat < ?)"
                ),
                vec![Value::from(cutoff), Value::from(cutoff)],
            ))
            .await?;

        let mut recovered = Vec::new();
        for row in stale {
            let id = row.str("id")?.to_string();
            let mut job: Job = serde_json::from_str(&row.str("payload")?)?;
            let previous_queue = job.queue.clone();

            job.retries += 1;
            job.updated_at = now;
            job.worker_id = None;
            if job.retries >= job.max_retries {
                job.status = JobStatus::DeadLetter;
                job.completed_at = Some(now);
                job.scheduled_at = None;
                job.queue = dead_letter_queue.to_string();
            } else {
                job.status = JobStatus::Retrying;
                job.scheduled_at = Some(now);
            }

            let affected = self
                .db
                .execute(&Stmt::new(
                    format!(
                        "UPDATE {TAB_JOBS} \
                         SET status = ?, retries = ?, scheduled_at = ?, completed_at = ?, \
                             updated_at = ?, worker_id = NULL, queue = ?, payload = ? \
                         WHERE id = ? AND status = 'running'"
                    ),
                    vec![
                        Value::from(job.status.as_db_str()),
                        Value::from(job.retries as i64),
                        ts(job.scheduled_at),
                        ts(job.completed_at),
                        Value::from(now.timestamp()),
                        Value::from(job.queue.as_str()),
                        json(&job)?,
                        Value::from(id.as_str()),
                    ],
                ))
                .await?;
            if affected.is_empty() {
                continue;
            }
            recovered.push(RecoveredJob {
                previous_queue,
                job,
            });
        }
        Ok(recovered)
    }

    /// Delete terminal jobs that finished before `before`, plus their
    /// dependency edges.
    ///
    /// Only terminal rows are removed: `pending`/`queued`/`running` jobs are
    /// never pruned, and a still-pending dependent is unaffected because its
    /// edge rows belong to the dependent, not to the pruned dependency.
    ///
    /// # Errors
    ///
    /// Fails when either delete cannot run.
    pub async fn purge_terminal_before(&self, before: DateTime<Utc>) -> Result<usize> {
        let mut tx = self.db.begin().await?;
        // Remove the edges of the rows about to disappear first: the sweep
        // below is only meaningful once the pruned jobs are gone.
        tx.execute(&Stmt::new(
            format!(
                "DELETE FROM {TAB_JOB_DEPENDENCIES} WHERE job_id IN ( \
                    SELECT id FROM {TAB_JOBS} \
                    WHERE status IN ('success', 'failed', 'cancelled', 'dead_letter') \
                      AND completed_at IS NOT NULL AND completed_at < ? \
                )"
            ),
            vec![Value::from(before.timestamp())],
        ))
        .await?;
        let affected = tx
            .execute(&Stmt::new(
                format!(
                    "DELETE FROM {TAB_JOBS} \
                     WHERE status IN ('success', 'failed', 'cancelled', 'dead_letter') \
                       AND completed_at IS NOT NULL AND completed_at < ?"
                ),
                vec![Value::from(before.timestamp())],
            ))
            .await?;
        // Edges owned by a job row that no longer exists (deleted by hand or by
        // an earlier sweep) would otherwise accumulate.
        tx.execute(&Stmt::new(
            format!(
                "DELETE FROM {TAB_JOB_DEPENDENCIES} WHERE job_id NOT IN (SELECT id FROM {TAB_JOBS})"
            ),
            vec![],
        ))
        .await?;
        tx.commit().await?;
        Ok(usize::try_from(affected.0).unwrap_or(usize::MAX))
    }

    /// Jobs of one queue, newest first.
    pub async fn list(
        &self,
        queue: &str,
        status: Option<JobStatus>,
        limit: usize,
    ) -> Result<Vec<Job>> {
        let (sql, params) = match status {
            Some(status) => (
                format!(
                    "SELECT payload FROM {TAB_JOBS} WHERE queue = ? AND status = ? \
                     ORDER BY created_at DESC LIMIT ?"
                ),
                vec![
                    Value::from(queue),
                    Value::from(status.as_db_str()),
                    Value::from(limit as i64),
                ],
            ),
            None => (
                format!(
                    "SELECT payload FROM {TAB_JOBS} WHERE queue = ? \
                     ORDER BY created_at DESC LIMIT ?"
                ),
                vec![Value::from(queue), Value::from(limit as i64)],
            ),
        };

        let rows = self.db.fetch_all(&Stmt::new(sql, params)).await?;

        let mut jobs = Vec::with_capacity(rows.len());
        for row in &rows {
            jobs.push(serde_json::from_str(&row.str("payload")?)?);
        }
        Ok(jobs)
    }
    /// How many jobs are still waiting to be claimed in `queue` (the
    /// database-only counterpart of the Redis sorted-set cardinality).
    ///
    /// # Errors
    ///
    /// Fails when the count query cannot reach the database.
    pub async fn queued_count(&self, queue: &str) -> Result<u64> {
        let row = self
            .db
            .fetch_one(&Stmt::new(
                format!(
                    "SELECT COUNT(*) as count FROM {TAB_JOBS} \
                     WHERE queue = ? AND status IN ('pending', 'retrying')"
                ),
                vec![Value::from(queue)],
            ))
            .await?;
        Ok(row.count("count"))
    }

    /// How many jobs of `queue` are running right now.
    ///
    /// The `processing` gauge used to read an in-memory map that was never
    /// written, so it was always `0` and not even scoped to a queue (JOB8).
    ///
    /// # Errors
    ///
    /// Fails when the count query cannot reach the database.
    pub async fn running_count(&self, queue: &str) -> Result<u64> {
        let row = self
            .db
            .fetch_one(&Stmt::new(
                format!(
                    "SELECT COUNT(*) as count FROM {TAB_JOBS} \
                     WHERE queue = ? AND status = 'running'"
                ),
                vec![Value::from(queue)],
            ))
            .await?;
        Ok(row.count("count"))
    }

    /// How many jobs each terminal status holds in `queue`.
    pub async fn terminal_counts(&self, queue: &str) -> Result<Vec<(String, u64)>> {
        let rows = self
            .db
            .fetch_all(&Stmt::new(
                format!(
                    "SELECT status, COUNT(*) as count FROM {TAB_JOBS} \
                     WHERE queue = ? GROUP BY status"
                ),
                vec![Value::from(queue)],
            ))
            .await?;

        rows.iter()
            .map(|row| Ok((row.str("status")?, row.count("count"))))
            .collect()
    }
}
