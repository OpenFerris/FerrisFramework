//! Query reports: definition, safe parameter binding and execution.

mod chart;
mod engine;
mod filters;
mod prepare;
mod types;

pub use engine::ReportEngine;
pub use filters::declared_filters;
pub use types::{
    ChartAggregate, ChartConfig, ChartData, ChartDataset, ChartType, PreparedReport,
    PreparedReportStatus, QueryReport, ReportColumn, ReportFilter, ReportResult,
};

#[cfg(test)]
mod tests;
