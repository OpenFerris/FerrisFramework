//! Ferris Report - query reports, filters and exports.
//!
//! Mirrors Frappe's **Query Report** and **Prepared Report** concepts without a
//! Python scripting layer: a report is a parameterised SQL query plus column and
//! filter metadata. Values are always bound, never interpolated.
//!
//! ```no_run
//! use ferris_report::{QueryReport, ReportColumn, ReportFilter, ReportEngine, ExportFormat};
//! # async fn demo(db: ferris_db::Db) -> ferris_core::Result<()> {
//! let report = QueryReport {
//!     name: "Open Todos".into(),
//!     doctype: "ToDo".into(),
//!     query: "SELECT name, title FROM tab_todo WHERE status = :status".into(),
//!     columns: vec![
//!         ReportColumn::new("name", "ID", "Data"),
//!         ReportColumn::new("title", "Title", "Data"),
//!     ],
//!     filters: vec![ReportFilter::new("status", "Status", "Data")],
//!     chart: None,
//! };
//! let engine = ReportEngine::new(db);
//! let result = engine.run(&report, &ferris_core::value!({ "status": "open" })).await?;
//! let csv = engine.export(&report, &ferris_core::value!({ "status": "open" }), ExportFormat::Csv).await?;
//! # Ok(()) }
//! ```

pub mod export;
#[cfg(feature = "http")]
pub mod http;
pub mod query;

pub use export::{ExportFormat, export_rows};
#[cfg(feature = "http")]
pub use http::ReportModule;
pub use query::{
    ChartAggregate, ChartConfig, ChartData, ChartDataset, ChartType, PreparedReport,
    PreparedReportStatus, QueryReport, ReportColumn, ReportEngine, ReportFilter, ReportResult,
};
