//! Persistence for prepared (async) reports.

use ferris_core::{Error, Result, ValidationError, Value};
use ferris_db::{
    Row, Stmt,
    sql::{Column, Ident, UpsertBuilder, UpsertSet},
};
use uuid::Uuid;

use super::engine::ReportEngine;
use super::types::{PreparedReport, PreparedReportStatus, QueryReport};

/// Table holding materialised reports.
const PREPARED: &str = "tab_prepared_report";

/// Every column of [`PREPARED`], in one place.
///
/// `created_at`/`updated_at` are epoch seconds; `result` and `error` are
/// nullable because a failed report stores the error instead of a result.
fn prepared_report_table() -> Vec<Column> {
    vec![
        Column::new("id", "VARCHAR(255)").not_null().primary_key(),
        Column::new("report", "VARCHAR(255)").not_null(),
        Column::new("filters", "TEXT").not_null(),
        Column::new("status", "VARCHAR(32)").not_null(),
        Column::new("result", "TEXT"),
        Column::new("error", "TEXT"),
        Column::new("created_at", "BIGINT").not_null(),
        Column::new("updated_at", "BIGINT").not_null(),
    ]
}

impl ReportEngine {
    /// Create the `tab_prepared_report` table used by [`ReportEngine::prepare`].
    pub async fn init(&self) -> Result<()> {
        self.db
            .ensure_table(PREPARED, &prepared_report_table())
            .await
    }

    /// Run a report and persist the result as a [`PreparedReport`].
    pub async fn prepare(&self, report: &QueryReport, filters: &Value) -> Result<PreparedReport> {
        let now = chrono::Utc::now();
        let mut prepared = PreparedReport {
            id: Uuid::new_v4().to_string(),
            report: report.name.clone(),
            filters: filters.clone(),
            status: PreparedReportStatus::Running,
            result: None,
            error: None,
            created_at: now,
            updated_at: now,
        };

        match self.run(report, filters).await {
            Ok(result) => {
                prepared.status = PreparedReportStatus::Completed;
                prepared.result = Some(result);
            }
            Err(e) => {
                prepared.status = PreparedReportStatus::Failed;
                prepared.error = Some(e.to_string());
            }
        }
        prepared.updated_at = chrono::Utc::now();
        self.save_prepared(&prepared).await?;
        if prepared.status == PreparedReportStatus::Failed {
            return Err(Error::Validation(ValidationError::Invalid(
                prepared
                    .error
                    .unwrap_or_else(|| "report failed".to_string()),
            )));
        }
        Ok(prepared)
    }

    /// Load a previously prepared report.
    pub async fn get_prepared(&self, id: &str) -> Result<Option<PreparedReport>> {
        let table = Ident::new(PREPARED)?.quote(self.db.dialect());
        let row = self
            .db
            .fetch_optional(&Stmt::new(
                format!("SELECT * FROM {table} WHERE id = ?"),
                vec![Value::from(id)],
            ))
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(prepared_from_row(&row)?))
    }

    /// The most recent prepared reports, newest first.
    ///
    /// Used by the host's `Prepared Report Analytics` observability mirror
    /// (SRV7): the report crate owns the rows, the host projects them onto its
    /// `System` doctypes without re-deriving the schema.
    ///
    /// # Errors
    ///
    /// Fails when the table cannot be read.
    // `Error` is the crate-wide error type; boxing it only for this read would
    // leak into the caller's signature for no benefit.
    #[allow(clippy::result_large_err)]
    pub async fn list_prepared(&self, limit: usize) -> Result<Vec<PreparedReport>> {
        let table = Ident::new(PREPARED)?.quote(self.db.dialect());
        let rows = self
            .db
            .fetch_all(&Stmt::new(
                format!("SELECT * FROM {table} ORDER BY created_at DESC LIMIT ?"),
                vec![Value::from(i64::try_from(limit).unwrap_or(i64::MAX))],
            ))
            .await?;
        rows.iter().map(prepared_from_row).collect()
    }

    async fn save_prepared(&self, prepared: &PreparedReport) -> Result<()> {
        let filters =
            serde_json::to_string(&prepared.filters).unwrap_or_else(|_| "null".to_string());
        let result = prepared
            .result
            .as_ref()
            .and_then(|r| serde_json::to_string(r).ok());
        let status = serde_json::to_string(&prepared.status)
            .unwrap_or_else(|_| "\"failed\"".to_string())
            .trim_matches('"')
            .to_string();

        let upsert = UpsertBuilder::new(self.db.dialect(), PREPARED)
            .values(vec![
                ("id", Value::from(prepared.id.as_str())),
                ("report", Value::from(prepared.report.as_str())),
                ("filters", Value::from(filters.as_str())),
                ("status", Value::from(status.as_str())),
                ("result", Value::from(result.clone())),
                ("error", Value::from(prepared.error.clone())),
                ("created_at", Value::from(prepared.created_at.timestamp())),
                ("updated_at", Value::from(prepared.updated_at.timestamp())),
            ])
            .conflict_on(["id"])
            .on_update("status", UpsertSet::Value(Value::from(status.as_str())))
            .on_update("result", UpsertSet::Value(Value::from(result)))
            .on_update(
                "error",
                UpsertSet::Value(Value::from(prepared.error.clone())),
            )
            .on_update(
                "updated_at",
                UpsertSet::Value(Value::from(prepared.updated_at.timestamp())),
            )
            .build()?;
        self.db.upsert(upsert).await.map(|_| ())
    }
}

fn timestamp_to_datetime(ts: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(ts, 0).unwrap_or_else(chrono::Utc::now)
}

/// Decode one `tab_prepared_report` row. Shared by [`ReportEngine::get_prepared`]
/// and [`ReportEngine::list_prepared`] so the two cannot disagree on status or
/// result decoding.
// `Error` is the crate-wide error type; see the note on `list_prepared`.
#[allow(clippy::result_large_err)]
fn prepared_from_row(row: &Row) -> Result<PreparedReport> {
    let status = row.str("status")?;
    Ok(PreparedReport {
        id: row.str("id")?,
        report: row.str("report")?,
        filters: row.json("filters")?,
        status: serde_json::from_str(&format!("\"{status}\""))
            .unwrap_or(PreparedReportStatus::Failed),
        result: row
            .opt_str("result")
            .and_then(|json| serde_json::from_str(&json).ok()),
        error: row.opt_str("error"),
        created_at: timestamp_to_datetime(row.i64("created_at")?),
        updated_at: timestamp_to_datetime(row.i64("updated_at")?),
    })
}
