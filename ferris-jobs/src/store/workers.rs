//! Worker liveness rows: the `job_workers` heartbeat and its typed read.
//!
//! Orphan recovery reads the heartbeat (`super::jobs::JobStore::recover_orphaned`),
//! so the writer and the reader of that table live next to each other.

use chrono::Utc;
use ferris_core::{Result, Value};
use ferris_db::{
    Stmt,
    sql::{UpsertBuilder, UpsertSet},
};

use super::columns::json;
use super::jobs::JobStore;
use super::schema::TAB_JOB_WORKERS;

/// One live worker row from `job_workers`, as the server's observability
/// mirror reports it (`RQ Worker`).
///
/// The queue keeps this table for liveness/orphan recovery; the host does not
/// know about it, so a typed read is the only honest way to expose it rather
/// than duplicating the column list in `ferris-server` (SRV7).
#[derive(Debug, Clone)]
pub struct JobWorker {
    pub worker_id: String,
    pub status: String,
    pub queues: Vec<String>,
    pub current_jobs: i64,
    /// Unix seconds.
    pub started_at: i64,
    /// Unix seconds.
    pub last_heartbeat: i64,
}

impl JobStore {
    /// Record a worker heartbeat, replacing any previous one.
    pub async fn heartbeat(
        &self,
        worker_id: &str,
        status: &str,
        queues: &[String],
        current_jobs: usize,
    ) -> Result<()> {
        let now = Utc::now().timestamp();
        let status = Value::from(status);
        let queues = json(&queues)?;
        let current = Value::from(current_jobs as i64);
        let heartbeat = Value::from(now);

        self.db
            .upsert(
                UpsertBuilder::new(self.db.dialect(), TAB_JOB_WORKERS)
                    .values(vec![
                        ("worker_id", Value::from(worker_id)),
                        ("status", status.clone()),
                        ("queues", queues.clone()),
                        ("current_jobs", current.clone()),
                        ("started_at", heartbeat.clone()),
                        ("last_heartbeat", heartbeat.clone()),
                    ])
                    .conflict_on(["worker_id"])
                    .on_update("status", UpsertSet::Value(status))
                    .on_update("queues", UpsertSet::Value(queues))
                    .on_update("current_jobs", UpsertSet::Value(current))
                    .on_update("last_heartbeat", UpsertSet::Value(heartbeat))
                    .build()?,
            )
            .await?;

        Ok(())
    }
    /// Every live worker row, for the `RQ Worker` observability mirror.
    ///
    /// Sorted by `worker_id` so a periodic sync produces a stable order. The
    /// `queues` column is a JSON array of queue names; a malformed value is
    /// reported as an empty list instead of failing the whole read, because a
    /// dashboard row is not worth dropping the rest of the snapshot for.
    ///
    /// # Errors
    ///
    /// Fails when the query cannot reach the database.
    // `Error` is the crate-wide error type; boxing it only for this read would
    // leak into the caller's signature for no benefit.
    #[allow(clippy::result_large_err)]
    pub async fn list_workers(&self) -> Result<Vec<JobWorker>> {
        let rows = self
            .db
            .fetch_all(&Stmt::new(
                format!("SELECT * FROM {TAB_JOB_WORKERS} ORDER BY worker_id"),
                vec![],
            ))
            .await?;

        let mut workers = Vec::with_capacity(rows.len());
        for row in &rows {
            let queues = row
                .json("queues")
                .ok()
                .and_then(|value| {
                    value.as_array().map(|items| {
                        items
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                })
                .unwrap_or_default();
            workers.push(JobWorker {
                worker_id: row.str("worker_id")?,
                status: row.str("status")?,
                queues,
                current_jobs: row.i64("current_jobs").unwrap_or(0),
                started_at: row.i64("started_at")?,
                last_heartbeat: row.i64("last_heartbeat")?,
            });
        }
        Ok(workers)
    }
}
