//! Operational endpoints that replace the stored snapshot DocTypes.
//!
//! `System Health Report`, `Database Storage Usage By Tables` and
//! `Prepared Report Analytics` were written into tables nothing read (and the
//! health one wrote a row every five minutes with no retention). The same data
//! is now computed on demand and served under the guarded API:
//!
//! * `GET /api/system/health` — a `SELECT 1` probe plus the running version.
//! * `GET /api/system/storage` — per-table rows and sizes, dialect-specific.
//! * `GET /api/report/prepared` — the prepared reports the engine already
//!   stores in `tab_prepared_report`, instead of a second copy.
//!
//! They are mounted with the rest of the API, so they require a session.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::router::AppState;

/// `GET /api/system/health` — the live probe, not a stored snapshot.
pub(super) async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(crate::observability::health_snapshot(&state.api.pool).await)
}

/// `GET /api/system/storage` — per-table row counts and sizes.
pub(super) async fn storage(State(state): State<AppState>) -> Response {
    match crate::observability::storage_snapshot(&state.api.pool).await {
        Ok(tables) => {
            let rows: Vec<serde_json::Value> = tables
                .into_iter()
                .map(|table| {
                    serde_json::json!({
                        "name": table.name,
                        "rows": table.rows,
                        "size_mb": table.size_mb,
                    })
                })
                .collect();
            Json(rows).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

/// `GET /api/report/prepared` — the prepared-report store, flattened.
#[cfg(feature = "report")]
pub(super) async fn prepared(State(state): State<AppState>) -> Response {
    let engine = ferris_report::ReportEngine::new(state.api.pool.clone());
    // The store is created lazily by the first preparation; a fresh install
    // asking for the list should get `[]`, not a missing-table error.
    if let Err(error) = engine.init().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response();
    }
    match engine.list_prepared(200).await {
        Ok(reports) => {
            let rows: Vec<serde_json::Value> = reports
                .iter()
                .map(|report| {
                    let row_count = report
                        .result
                        .as_ref()
                        .map_or(0, ferris_report::ReportResult::total);
                    let duration_ms = (report.updated_at - report.created_at)
                        .num_milliseconds()
                        .max(0);
                    serde_json::json!({
                        "id": report.id,
                        "report": report.report,
                        "status": match report.status {
                            ferris_report::PreparedReportStatus::Queued => "Queued",
                            ferris_report::PreparedReportStatus::Running => "Running",
                            ferris_report::PreparedReportStatus::Completed => "Completed",
                            ferris_report::PreparedReportStatus::Failed => "Failed",
                        },
                        "row_count": row_count,
                        "duration_ms": duration_ms,
                        "created_at": report.created_at.to_rfc3339(),
                        "updated_at": report.updated_at.to_rfc3339(),
                    })
                })
                .collect();
            Json(rows).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}
