//! Report, column, filter and chart wire types.

use ferris_core::Value;
use serde::{Deserialize, Serialize};

/// A report column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportColumn {
    pub fieldname: String,
    pub label: String,
    pub fieldtype: String,
    pub width: Option<u32>,
}

impl ReportColumn {
    pub fn new(
        fieldname: impl Into<String>,
        label: impl Into<String>,
        fieldtype: impl Into<String>,
    ) -> Self {
        Self {
            fieldname: fieldname.into(),
            label: label.into(),
            fieldtype: fieldtype.into(),
            width: None,
        }
    }
}

/// A report filter, bound to `:fieldname` placeholders in the query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportFilter {
    pub fieldname: String,
    pub label: String,
    pub fieldtype: String,
    pub default: Option<Value>,
    pub options: Option<String>,
    pub reqd: bool,
}

impl ReportFilter {
    pub fn new(
        fieldname: impl Into<String>,
        label: impl Into<String>,
        fieldtype: impl Into<String>,
    ) -> Self {
        Self {
            fieldname: fieldname.into(),
            label: label.into(),
            fieldtype: fieldtype.into(),
            default: None,
            options: None,
            reqd: false,
        }
    }

    pub fn with_default(mut self, value: Value) -> Self {
        self.default = Some(value);
        self
    }

    pub fn required(mut self) -> Self {
        self.reqd = true;
        self
    }
}

/// Chart type for a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartType {
    Line,
    Bar,
    Pie,
    Percentage,
    Donut,
}

/// How to aggregate values within a chart group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartAggregate {
    Count,
    #[default]
    Sum,
    Average,
    Min,
    Max,
}

/// Optional chart configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartConfig {
    pub chart_type: ChartType,
    pub x_field: String,
    /// Value fields to aggregate. Empty means "count rows per label".
    pub y_fields: Vec<String>,
    #[serde(default)]
    pub aggregate: Option<ChartAggregate>,
}

/// One plotted series.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartDataset {
    pub name: String,
    pub values: Vec<f64>,
}

/// Chart-ready data derived from a [`ReportResult`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartData {
    pub chart_type: ChartType,
    pub labels: Vec<String>,
    pub datasets: Vec<ChartDataset>,
}

/// A parameterised SQL query report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryReport {
    pub name: String,
    pub doctype: String,
    /// SQL using `:fieldname` placeholders. Never interpolate filter values.
    pub query: String,
    pub columns: Vec<ReportColumn>,
    pub filters: Vec<ReportFilter>,
    pub chart: Option<ChartConfig>,
}

/// Materialised report data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportResult {
    pub columns: Vec<ReportColumn>,
    pub rows: Vec<Vec<Value>>,
}

impl ReportResult {
    pub fn total(&self) -> usize {
        self.rows.len()
    }
}

/// Status of a prepared (async) report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparedReportStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

/// A generated report persisted for later retrieval (Frappe's `Prepared Report`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedReport {
    pub id: String,
    pub report: String,
    pub filters: Value,
    pub status: PreparedReportStatus,
    pub result: Option<ReportResult>,
    pub error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
