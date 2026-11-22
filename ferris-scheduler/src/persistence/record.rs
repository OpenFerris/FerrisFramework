//! `ScheduleRecord`: the row shape of a schedule and the row <-> model
//! conversion.
//!
//! Every stored value is validated here. A corrupt timestamp used to become
//! `Utc::now()`, which made a garbage `next_run` look freshly scheduled and
//! silently disabled the missed-run recovery (SCH7).

use crate::schedule::{Schedule, ScheduleConfig};
use chrono::{DateTime, Utc};
use ferris_core::{Error, Result, ValidationError, Value};
use ferris_db::Row;
use serde::de::DeserializeOwned;

pub struct ScheduleRecord {
    pub id: String,
    pub name: String,
    pub cron_expression: String,
    pub job_name: String,
    pub payload: String,
    pub queue: String,
    pub priority: i32,
    pub timeout_secs: i64,
    pub max_retries: i32,
    pub tags: String,
    pub metadata: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_run: Option<String>,
    pub next_run: Option<String>,
    pub run_count: i64,
    pub failure_count: i64,
    pub last_error: Option<String>,
}

/// A stored timestamp, or a typed error naming the corrupt column.
///
/// The old helper fell back to `Utc::now()`, so a corrupt `next_run` looked
/// freshly scheduled and `missed_while_down` never fired — the exact silent
/// loss the A12 recovery exists to prevent (SCH7).
fn parse_dt(value: &str, column: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| {
            Error::Validation(ValidationError::Invalid(format!(
                "schedules.{column} is not an RFC 3339 timestamp ({value:?}): {e}"
            )))
        })
}

fn parse_opt_dt(value: Option<String>, column: &str) -> Result<Option<DateTime<Utc>>> {
    value
        .as_deref()
        .map(|value| parse_dt(value, column))
        .transpose()
}

/// A JSON column, decoded or reported with the column that failed.
fn parse_json<T: DeserializeOwned>(value: &str, column: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|e| {
        Error::Validation(ValidationError::Invalid(format!(
            "schedules.{column} is not valid JSON ({value:?}): {e}"
        )))
    })
}

/// The integer form of a priority, and back.
fn priority_from_i32(value: i32) -> Result<ferris_jobs::JobPriority> {
    match value {
        -10 => Ok(ferris_jobs::JobPriority::Low),
        0 => Ok(ferris_jobs::JobPriority::Normal),
        10 => Ok(ferris_jobs::JobPriority::High),
        20 => Ok(ferris_jobs::JobPriority::Critical),
        other => Err(Error::Validation(ValidationError::Invalid(format!(
            "schedules.priority has unknown value {other}"
        )))),
    }
}

fn priority_to_i32(priority: ferris_jobs::JobPriority) -> i32 {
    match priority {
        ferris_jobs::JobPriority::Low => -10,
        ferris_jobs::JobPriority::Normal => 0,
        ferris_jobs::JobPriority::High => 10,
        ferris_jobs::JobPriority::Critical => 20,
    }
}

impl ScheduleRecord {
    /// Read a record out of a result row.
    ///
    /// Hand-written rather than derived: the three backends disagree on the
    /// width of an integer and on whether a boolean arrives as a bool, and
    /// [`Row`] already resolves both.
    pub(super) fn from_row(row: &Row) -> Result<Self> {
        Ok(Self {
            id: row.str("id")?,
            name: row.str("name")?,
            cron_expression: row.str("cron_expression")?,
            job_name: row.str("job_name")?,
            payload: row.str("payload")?,
            queue: row.str("queue")?,
            priority: row.opt_i64("priority").unwrap_or(0) as i32,
            timeout_secs: row.opt_i64("timeout_secs").unwrap_or(300),
            max_retries: row.opt_i64("max_retries").unwrap_or(3) as i32,
            tags: row.str("tags")?,
            metadata: row.str("metadata")?,
            enabled: row.opt_bool("enabled").unwrap_or(false),
            created_at: row.str("created_at")?,
            updated_at: row.str("updated_at")?,
            last_run: row.opt_str("last_run"),
            next_run: row.opt_str("next_run"),
            run_count: row.opt_i64("run_count").unwrap_or(0),
            failure_count: row.opt_i64("failure_count").unwrap_or(0),
            last_error: row.opt_str("last_error"),
        })
    }

    /// Convert to Schedule
    ///
    /// # Errors
    ///
    /// Fails when a stored timestamp, JSON column or priority is corrupt.
    pub fn into_schedule(self) -> Result<Schedule> {
        Ok(Schedule {
            id: self.id,
            name: self.name,
            config: ScheduleConfig {
                cron_expression: self.cron_expression,
                job_name: self.job_name,
                payload: parse_json(&self.payload, "payload")?,
                queue: self.queue,
                priority: priority_from_i32(self.priority)?,
                timeout: std::time::Duration::from_secs(self.timeout_secs as u64),
                max_retries: self.max_retries as u32,
                tags: parse_json(&self.tags, "tags")?,
                metadata: parse_json(&self.metadata, "metadata")?,
            },
            enabled: self.enabled,
            created_at: parse_dt(&self.created_at, "created_at")?,
            updated_at: parse_dt(&self.updated_at, "updated_at")?,
            last_run: parse_opt_dt(self.last_run, "last_run")?,
            next_run: parse_opt_dt(self.next_run, "next_run")?,
            run_count: self.run_count as u64,
            failure_count: self.failure_count as u64,
            last_error: self.last_error,
        })
    }

    /// The bind values for the `INSERT` half of [`ScheduleStore::save`].
    ///
    /// Each is bound twice on the upsert path, so they are produced once here
    /// rather than listed twice at the call site.
    pub(super) fn values(&self) -> Vec<(&'static str, Value)> {
        vec![
            ("id", Value::from(self.id.as_str())),
            ("name", Value::from(self.name.as_str())),
            (
                "cron_expression",
                Value::from(self.cron_expression.as_str()),
            ),
            ("job_name", Value::from(self.job_name.as_str())),
            ("payload", Value::from(self.payload.as_str())),
            ("queue", Value::from(self.queue.as_str())),
            ("priority", Value::from(self.priority)),
            ("timeout_secs", Value::from(self.timeout_secs)),
            ("max_retries", Value::from(self.max_retries)),
            ("tags", Value::from(self.tags.as_str())),
            ("metadata", Value::from(self.metadata.as_str())),
            ("enabled", Value::from(i64::from(self.enabled))),
            ("created_at", Value::from(self.created_at.as_str())),
            ("updated_at", Value::from(self.updated_at.as_str())),
            ("last_run", opt_str(&self.last_run)),
            ("next_run", opt_str(&self.next_run)),
            ("run_count", Value::from(self.run_count)),
            ("failure_count", Value::from(self.failure_count)),
            ("last_error", opt_str(&self.last_error)),
        ]
    }

    /// Create from Schedule
    ///
    /// # Errors
    ///
    /// Fails when the payload, tags or metadata cannot be encoded.
    pub fn from_schedule(schedule: &Schedule) -> Result<Self> {
        Ok(Self {
            id: schedule.id.clone(),
            name: schedule.name.clone(),
            cron_expression: schedule.config.cron_expression.clone(),
            job_name: schedule.config.job_name.clone(),
            payload: encode_json(&schedule.config.payload, "payload")?,
            queue: schedule.config.queue.clone(),
            priority: priority_to_i32(schedule.config.priority),
            timeout_secs: schedule.config.timeout.as_secs() as i64,
            max_retries: schedule.config.max_retries as i32,
            tags: encode_json(&schedule.config.tags, "tags")?,
            metadata: encode_json(&schedule.config.metadata, "metadata")?,
            enabled: schedule.enabled,
            created_at: schedule.created_at.to_rfc3339(),
            updated_at: schedule.updated_at.to_rfc3339(),
            last_run: schedule.last_run.map(|d| d.to_rfc3339()),
            next_run: schedule.next_run.map(|d| d.to_rfc3339()),
            run_count: schedule.run_count as i64,
            failure_count: schedule.failure_count as i64,
            last_error: schedule.last_error.clone(),
        })
    }
}

fn encode_json<T: serde::Serialize>(value: &T, column: &str) -> Result<String> {
    serde_json::to_string(value).map_err(|e| {
        Error::Validation(ValidationError::Invalid(format!(
            "schedules.{column} cannot be encoded: {e}"
        )))
    })
}

fn opt_str(value: &Option<String>) -> Value {
    value.as_deref().map_or(Value::Null, Value::from)
}
