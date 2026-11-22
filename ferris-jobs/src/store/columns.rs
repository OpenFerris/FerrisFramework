//! Row <-> [`Job`] conversion: the single place that decides which columns a
//! job has and how each value is encoded.
//!
//! `persist` (insert/upsert) and `update_guarded` (conditional update) both
//! build their statements from [`job_columns`], so a column added there is
//! written by both instead of one of them silently omitting it (JOB2/JOB5).

use chrono::{DateTime, Utc};
use ferris_core::{Error, JobError, Result, Value};
use serde::Serialize;

use crate::job::Job;

/// An optional timestamp column: `NULL` until the job reaches that stage.
pub(super) fn ts(value: Option<DateTime<Utc>>) -> Value {
    value.map_or(Value::Null, |dt| Value::from(dt.timestamp()))
}

/// A JSON column, stored as its serialised text.
///
/// Encoding failure is reported, not written as SQL `NULL`: `payload` is the
/// document every read path decodes, so a silent `NULL` turned an encoding bug
/// into an undecodable row with no clue why (JOB10).
pub(super) fn json<T: Serialize>(value: &T) -> Result<Value> {
    serde_json::to_string(value)
        .map(|text| Value::from(text.as_str()))
        .map_err(|e| Error::Job(JobError::Failed(format!("job encoding failed: {e}"))))
}

/// Every per-job column, in table order.
///
/// One function so `persist` (insert/upsert) and `update_guarded` (conditional
/// update) cannot disagree about which columns exist: a column added here is
/// written by both (JOB2/JOB5).
pub(super) fn job_columns(job: &Job) -> Result<Vec<(&'static str, Value)>> {
    let result = match &job.result {
        Some(result) => json(result)?,
        None => Value::Null,
    };
    Ok(vec![
        ("id", Value::from(job.id.as_str())),
        ("name", Value::from(job.name.as_str())),
        ("queue", Value::from(job.queue.as_str())),
        ("payload", json(job)?),
        ("priority", Value::from(job.priority as i64)),
        ("status", Value::from(job.status.as_db_str())),
        ("retries", Value::from(job.retries as i64)),
        ("max_retries", Value::from(job.max_retries as i64)),
        ("timeout_secs", Value::from(job.timeout.as_secs() as i64)),
        ("scheduled_at", ts(job.scheduled_at)),
        ("started_at", ts(job.started_at)),
        ("completed_at", ts(job.completed_at)),
        ("depends_on", json(&job.depends_on)?),
        ("tags", json(&job.tags)?),
        ("metadata", json(&job.metadata)?),
        ("created_at", Value::from(job.created_at.timestamp())),
        ("updated_at", Value::from(job.updated_at.timestamp())),
        (
            "worker_id",
            job.worker_id.as_deref().map_or(Value::Null, Value::from),
        ),
        ("result", result),
    ])
}

/// Columns whose value may change after the row is created.
///
/// `created_at` is deliberately absent: it records when the job was created and
/// must not move when the row is updated. Everything else is mutable — omitting
/// `scheduled_at` here is what made retry backoff inert (JOB2).
pub(super) const MUTABLE_COLUMNS: &[&str] = &[
    "name",
    "queue",
    "payload",
    "priority",
    "status",
    "retries",
    "max_retries",
    "timeout_secs",
    "scheduled_at",
    "started_at",
    "completed_at",
    "depends_on",
    "tags",
    "metadata",
    "updated_at",
    "worker_id",
    "result",
];
