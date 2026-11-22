//! `ScheduleStore`: the `schedules` table and its statements.

use super::record::ScheduleRecord;
use crate::schedule::Schedule;
use chrono::{DateTime, Utc};
use ferris_core::{Error, Result, Value};
use ferris_db::{
    Db, Stmt,
    sql::{Column, UpsertBuilder, UpsertSet},
};
use tracing::warn;

pub(super) const TAB_SCHEDULES: &str = "schedules";

/// `enabled` is a 0/1 integer rather than a dialect boolean: SQLite has no
/// boolean type and MySQL's `BOOLEAN` is a synonym for `TINYINT(1)`, so
/// [`Row::opt_bool`] reads back whichever the backend produced.
fn schedules_table() -> Vec<Column> {
    vec![
        Column::new("id", "VARCHAR(255)").not_null().primary_key(),
        Column::new("name", "VARCHAR(255)").not_null(),
        Column::new("cron_expression", "VARCHAR(255)").not_null(),
        Column::new("job_name", "VARCHAR(255)").not_null(),
        Column::new("payload", "TEXT").not_null(),
        Column::new("queue", "VARCHAR(255)")
            .not_null()
            .default("'default'"),
        Column::new("priority", "INTEGER").not_null().default("0"),
        Column::new("timeout_secs", "INTEGER")
            .not_null()
            .default("300"),
        Column::new("max_retries", "INTEGER")
            .not_null()
            .default("3"),
        Column::new("tags", "TEXT").not_null().default("'[]'"),
        Column::new("metadata", "TEXT").not_null().default("'{}'"),
        Column::new("enabled", "INTEGER").not_null().default("1"),
        Column::new("created_at", "VARCHAR(64)").not_null(),
        Column::new("updated_at", "VARCHAR(64)").not_null(),
        Column::new("last_run", "VARCHAR(64)"),
        Column::new("next_run", "VARCHAR(64)"),
        Column::new("run_count", "BIGINT").not_null().default("0"),
        Column::new("failure_count", "BIGINT")
            .not_null()
            .default("0"),
        Column::new("last_error", "TEXT"),
    ]
}

/// Every column of `schedules`, for the reads that do not filter.
///
/// The store is the only writer, so naming them once here means a column added
/// below cannot be silently forgotten by one of the `SELECT`s.
const RECORD_COLUMNS: &str = "id, name, cron_expression, job_name, payload, queue, \
     priority, timeout_secs, max_retries, tags, metadata, enabled, created_at, \
     updated_at, last_run, next_run, run_count, failure_count, last_error";

/// Schedule database record
pub struct ScheduleStore {
    pub(super) db: Db,
}

impl ScheduleStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Initialize database tables
    pub async fn init(&self) -> Result<()> {
        self.db
            .ensure_table(TAB_SCHEDULES, &schedules_table())
            .await
    }

    /// Save schedule
    ///
    /// # Errors
    ///
    /// Fails when the record cannot be encoded or written.
    pub async fn save(&self, schedule: &Schedule) -> Result<()> {
        let record = ScheduleRecord::from_schedule(schedule)?;
        let values = record.values();

        // `created_at` is deliberately absent from the update set: the row
        // keeps the moment it was first written, and `updated_at` carries the
        // later one.
        let mut builder = UpsertBuilder::new(self.db.dialect(), TAB_SCHEDULES)
            .values(values.clone())
            .conflict_on(["id"]);
        for (column, value) in values {
            if column != "id" && column != "created_at" {
                builder = builder.on_update(column, UpsertSet::Value(value));
            }
        }

        self.db.upsert(builder.build()?).await?;
        Ok(())
    }

    /// Get schedule by ID
    ///
    /// # Errors
    ///
    /// Fails when the row cannot be read or is corrupt.
    pub async fn get(&self, id: &str) -> Result<Option<Schedule>> {
        let row = self
            .db
            .fetch_optional(&Stmt::new(
                format!("SELECT {RECORD_COLUMNS} FROM {TAB_SCHEDULES} WHERE id = ?"),
                vec![Value::from(id)],
            ))
            .await?;

        row.map(|row| ScheduleRecord::from_row(&row)?.into_schedule())
            .transpose()
    }

    /// List all schedules.
    ///
    /// A corrupt row is skipped with a warning naming its id and the failing
    /// column; it used to fail the whole read, so one bad row prevented the
    /// scheduler from booting and every healthy schedule stopped running. The
    /// row is not silent — the warning is the trace an operator needs to fix
    /// it — and the rest of the table keeps working.
    ///
    /// # Errors
    ///
    /// Fails only when the table itself cannot be read.
    pub async fn list(&self) -> Result<Vec<Schedule>> {
        let rows = self
            .db
            .fetch_all(&Stmt::new(
                format!("SELECT {RECORD_COLUMNS} FROM {TAB_SCHEDULES} ORDER BY created_at DESC"),
                vec![],
            ))
            .await?;

        let mut schedules = Vec::with_capacity(rows.len());
        for row in &rows {
            let id = row.opt_str("id").unwrap_or_default();
            match ScheduleRecord::from_row(row).and_then(ScheduleRecord::into_schedule) {
                Ok(schedule) => schedules.push(schedule),
                Err(e) => warn!("Skipping corrupt schedule row {id:?}: {e}"),
            }
        }
        Ok(schedules)
    }

    /// Delete schedule
    ///
    /// # Errors
    ///
    /// Fails when the delete cannot run.
    pub async fn delete(&self, id: &str) -> Result<()> {
        self.db
            .execute(&Stmt::new(
                format!("DELETE FROM {TAB_SCHEDULES} WHERE id = ?"),
                vec![Value::from(id)],
            ))
            .await?;
        Ok(())
    }

    /// Update schedule run info
    ///
    /// A successful run clears the failure counter and the stored error; a
    /// failed one increments the counter and records the error. The error
    /// placeholder only exists in the failing branch, so it is bound there too.
    ///
    /// # Errors
    ///
    /// Fails when the update cannot run, or when no row carries `id`.
    pub async fn update_run_info(
        &self,
        id: &str,
        last_run: DateTime<Utc>,
        next_run: Option<DateTime<Utc>>,
        success: bool,
        error: Option<String>,
    ) -> Result<()> {
        let mut params = vec![
            Value::from(last_run.to_rfc3339()),
            next_run.map_or(Value::Null, |d| Value::from(d.to_rfc3339())),
            Value::from(Utc::now().to_rfc3339()),
        ];
        let outcome = if success {
            "failure_count = 0, last_error = NULL"
        } else {
            params.push(error.map_or(Value::Null, |e| Value::from(e.as_str())));
            "failure_count = failure_count + 1, last_error = ?"
        };
        params.push(Value::from(id));

        let affected = self
            .db
            .execute(&Stmt::new(
                format!(
                    "UPDATE {TAB_SCHEDULES} SET \
                        last_run = ?, next_run = ?, run_count = run_count + 1, \
                        updated_at = ?, {outcome} \
                     WHERE id = ?"
                ),
                params,
            ))
            .await?;

        // A run recorded against a schedule that no longer exists is a defect,
        // not a success: the counters the operator sees would stay frozen
        // forever (SCH13).
        if affected.is_empty() {
            return Err(Error::Job(ferris_core::JobError::NotFound(id.to_string())));
        }
        Ok(())
    }

    /// Test hook: remove the backing table so the next write fails.
    ///
    /// Used to prove that a persist failure never leaves a cron job registered
    /// with the engine (SCH11).
    #[cfg(test)]
    pub(crate) async fn drop_table_for_test(&self) -> Result<()> {
        self.db
            .execute_script(&format!("DROP TABLE {TAB_SCHEDULES}"))
            .await
    }
}
