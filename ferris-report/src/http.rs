//! Report endpoints: run and export a saved `Report` (Query Report).
//!
//! A `Report` document holds a parameterised SQL query (`:fieldname`
//! placeholders) and its source `ref_doctype`. Running one is gated twice:
//!
//! 1. the `Report` row is read as the caller (`Permission::Read`), and
//! 2. the caller needs [`Permission::Report`] on the declared source DocType —
//!    the same right Frappe uses to decide who may run a report over a DocType.
//!
//! The engine then validates the stored SQL as a **single read-only statement**,
//! wraps it in a row cap and runs it under a deadline (`ReportEngine::run`), so
//! a "report" cannot mutate data or materialise a table without bound. That
//! read-only check is the guard: the engine's read guard proves the caller may
//! read the *declared* source DocType, but it is not a table-level sandbox — a
//! query that names another table can still read it, which is why the report
//! row itself is administrator-authored.
//!
//! Filters are **derived from the query's placeholders** (`:status` becomes a
//! filter named `status`), because the `Report` DocType has no separate filter
//! declaration yet. That means a client may pass exactly the placeholders the
//! stored SQL declares and nothing else; an undeclared key is a 400.
//!
//! There is deliberately **no prepared-report route**: the engine's `prepare`
//! is synchronous and the async `Prepared Report` state machine does not exist,
//! so the endpoint is not mounted (404) instead of pretending a background job
//! was queued.

use std::sync::Arc;

use crate::{ExportFormat, QueryReport, ReportEngine, ReportResult, query::declared_filters};
use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use ferris_auth::middleware::AuthenticatedUser;
use ferris_core::{
    AuthzError, Error, Permission, Resource, Result, ValidationError, Value,
    permissions::check_permission,
};
use ferris_document::{DocumentEngine, EngineContext, ListQuery};
use serde::Deserialize;
use serde_json::json;

use ferris_api::{ApiState, ModuleState, OpenApiFragment};

/// The `DocType` holding saved report definitions.
const REPORT: &str = "Report";

/// JSON body of `POST /api/report/{name}/run`.
#[derive(Debug, Default, Deserialize)]
pub struct RunRequest {
    /// Values for the query's `:placeholders`. Undeclared keys are rejected.
    #[serde(default)]
    pub filters: Option<Value>,
}

/// Query string of `GET /api/report/{name}/export`.
#[derive(Debug, Default, Deserialize)]
pub struct ExportQuery {
    /// `csv` (default), `tsv`, `md`/`markdown`, `html` or `json`.
    #[serde(default)]
    pub format: Option<String>,
    /// Filters as a JSON object in one query parameter
    /// (`?filters=%7B%22status%22%3A%22open%22%7D`).
    #[serde(default)]
    pub filters: Option<String>,
}

/// `POST /api/report/{name}/run`.
pub async fn run(
    State(state): State<Arc<ApiState>>,
    user: AuthenticatedUser,
    Path(name): Path<String>,
    Json(request): Json<RunRequest>,
) -> Response {
    let filters = request.filters.unwrap_or_else(empty_filters);
    match run_report(&state, &user.0, &name, &filters).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => ferris_api::router::error_response(error),
    }
}

/// `GET /api/report/{name}/export?format=...`.
pub async fn export(
    State(state): State<Arc<ApiState>>,
    user: AuthenticatedUser,
    Path(name): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Response {
    let format = match parse_export_format(query.format.as_deref()) {
        Ok(format) => format,
        Err(error) => return ferris_api::router::error_response(error),
    };
    let filters = match query.filters.as_deref() {
        Some(raw) => match serde_json::from_str::<Value>(raw) {
            Ok(value) => value,
            Err(error) => {
                return ferris_api::router::error_response(Error::Validation(
                    ValidationError::Invalid(format!("`filters` is not valid JSON: {error}")),
                ));
            }
        },
        None => empty_filters(),
    };
    match export_report(&state, &user.0, &name, &filters, format).await {
        Ok(bytes) => file_response(bytes, &name, format),
        Err(error) => ferris_api::router::error_response(error),
    }
}

/// `GET /api/report` — the reports the caller may read.
pub async fn list(State(state): State<Arc<ApiState>>, user: AuthenticatedUser) -> Response {
    match available_reports(&state, &user.0).await {
        Ok(reports) => Json(reports).into_response(),
        Err(error) => ferris_api::router::error_response(error),
    }
}

/// Run a saved report as `user`.
///
/// # Errors
///
/// A permission error when the caller cannot read the `Report` row or lacks
/// `Report` on its source DocType, a validation error for an unsupported report
/// type or a malformed/undeclared filter, or the database error of the query.
pub(crate) async fn run_report(
    state: &ApiState,
    user: &ferris_core::User,
    name: &str,
    filters: &Value,
) -> Result<ReportResult> {
    let (report, engine) = prepared_report(state, user, name).await?;
    engine.run(&report, filters).await
}

/// Export a saved report as `user`.
///
/// # Errors
///
/// Same as [`run_report`], plus the export writer's serialization error.
pub(crate) async fn export_report(
    state: &ApiState,
    user: &ferris_core::User,
    name: &str,
    filters: &Value,
    format: ExportFormat,
) -> Result<Vec<u8>> {
    let (report, engine) = prepared_report(state, user, name).await?;
    engine.export(&report, filters, format).await
}

/// What `GET /api/report` returns: the definition fields a caller needs to
/// choose a report, not the SQL.
pub(crate) async fn available_reports(
    state: &ApiState,
    user: &ferris_core::User,
) -> Result<Vec<Value>> {
    let meta = ferris_api::store::effective_meta(&state.pool, REPORT).await?;
    let mut ctx = EngineContext::new(REPORT.to_string(), "");
    ctx.meta = meta.as_ref();
    ctx.user = Some(user.clone());
    let query = ListQuery {
        limit: Some(200),
        order_by: Some("modified desc".to_string()),
        ..Default::default()
    };
    let (docs, _) = DocumentEngine::new(state.pool.clone())
        .list(&ctx, &query)
        .await?;
    Ok(docs
        .iter()
        .map(|doc| {
            Value::from_json(json!({
                "name": doc_str(doc, "name"),
                "report_name": doc_str(doc, "report_name"),
                "ref_doctype": doc_str(doc, "ref_doctype"),
                "report_type": doc_str(doc, "report_type"),
            }))
        })
        .collect())
}

/// Load the `Report` row and turn it into the engine's definition + engine.
async fn prepared_report(
    state: &ApiState,
    user: &ferris_core::User,
    name: &str,
) -> Result<(QueryReport, ReportEngine)> {
    let doc = load_report(state, user, name).await?;
    let report = query_report(name, &doc)?;
    if !check_permission(
        user,
        &Resource::new(report.doctype.clone(), ""),
        Permission::Report,
    ) {
        return Err(Error::Authz(AuthzError::PermissionDenied {
            action: Permission::Report.as_str().to_string(),
            resource: report.doctype.clone(),
        }));
    }
    let report_engine = report_engine(state, user);
    Ok((report, report_engine))
}

async fn load_report(state: &ApiState, user: &ferris_core::User, name: &str) -> Result<Value> {
    let meta = ferris_api::store::effective_meta(&state.pool, REPORT).await?;
    let mut ctx = EngineContext::new(REPORT.to_string(), "");
    ctx.meta = meta.as_ref();
    ctx.user = Some(user.clone());
    DocumentEngine::new(state.pool.clone())
        .get(&ctx, name)
        .await
}

/// Validate the stored definition and map it onto [`QueryReport`].
fn query_report(name: &str, doc: &Value) -> Result<QueryReport> {
    let ref_doctype = doc_str(doc, "ref_doctype");
    if ref_doctype.trim().is_empty() {
        return Err(Error::Validation(ValidationError::Required(
            "ref_doctype".to_string(),
        )));
    }
    let report_type = doc_str(doc, "report_type");
    if !report_type.trim().is_empty() && !report_type.eq_ignore_ascii_case("Query Report") {
        return Err(unsupported(format!(
            "report type `{report_type}` (only Query Report is executed)"
        )));
    }
    if !doc_str(doc, "script").trim().is_empty() {
        return Err(unsupported(
            "report `script` (Python scripts are not executed)",
        ));
    }
    let query = doc_str(doc, "query");
    if query.trim().is_empty() {
        return Err(unsupported("report has no SQL query"));
    }
    Ok(QueryReport {
        name: name.to_string(),
        doctype: ref_doctype,
        // Filters are the query's own placeholders; an undeclared client key is
        // rejected by `ReportEngine::run` rather than smuggled into the binder.
        filters: declared_filters(&query),
        query,
        columns: Vec::new(),
        chart: None,
    })
}

/// The engine's own guard: `Report` on the declared source DocType, evaluated
/// as the same caller the HTTP path authorized.
fn report_engine(state: &ApiState, user: &ferris_core::User) -> ReportEngine {
    let caller = user.clone();
    let guard: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |doctype: &str| {
        check_permission(&caller, &Resource::new(doctype, ""), Permission::Report)
    });
    ReportEngine::with_read_guard(state.pool.clone(), Some(guard))
}

fn parse_export_format(raw: Option<&str>) -> Result<ExportFormat> {
    match raw.unwrap_or("csv").trim().to_ascii_lowercase().as_str() {
        "csv" => Ok(ExportFormat::Csv),
        "tsv" => Ok(ExportFormat::Tsv),
        "md" | "markdown" => Ok(ExportFormat::Markdown),
        "html" => Ok(ExportFormat::Html),
        "json" => Ok(ExportFormat::Json),
        other => Err(Error::Validation(ValidationError::Invalid(format!(
            "unsupported report export format `{other}`; expected csv, tsv, md, html or json"
        )))),
    }
}

fn file_response(bytes: Vec<u8>, name: &str, format: ExportFormat) -> Response {
    let (content_type, extension) = match format {
        ExportFormat::Csv => ("text/csv; charset=utf-8", "csv"),
        ExportFormat::Tsv => ("text/tab-separated-values; charset=utf-8", "tsv"),
        ExportFormat::Markdown => ("text/markdown; charset=utf-8", "md"),
        ExportFormat::Html => ("text/html; charset=utf-8", "html"),
        ExportFormat::Json => ("application/json", "json"),
    };
    let filename = format!("{}.{extension}", sanitize_filename(name));
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

fn empty_filters() -> Value {
    Value::Object(Default::default())
}

fn unsupported(what: impl std::fmt::Display) -> Error {
    Error::Validation(ValidationError::Invalid(format!(
        "unsupported report: {what}"
    )))
}

fn doc_str(doc: &Value, key: &str) -> String {
    doc.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Keep a report name usable as a file name.
fn sanitize_filename(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "report".to_string()
    } else {
        out
    }
}

/// The report feature as a native module.
///
/// Routes are relative to the default `/api` mount; the host applies the same
/// auth and CSRF layers as the generic API and merges the `OpenAPI` fragment.
#[derive(Debug, Default, Clone, Copy)]
pub struct ReportModule;

impl ferris_api::Module for ReportModule {
    fn name(&self) -> &'static str {
        "report"
    }

    fn routes(&self) -> axum::Router<ModuleState> {
        axum::Router::new()
            .route("/report", axum::routing::get(list))
            .route("/report/{name}/run", axum::routing::post(run))
            .route("/report/{name}/export", axum::routing::get(export))
    }

    fn openapi(&self) -> OpenApiFragment {
        OpenApiFragment {
            paths: vec![
                (
                    "/api/report".to_string(),
                    json!({
                        "get": {
                            "summary": "List the reports the caller may read",
                            "tags": ["report"],
                            "responses": {
                                "200": { "description": "Report definitions" }
                            }
                        }
                    }),
                ),
                (
                    "/api/report/{name}/run".to_string(),
                    json!({
                        "post": {
                            "summary": "Run a saved query report",
                            "tags": ["report"],
                            "parameters": [path_param("name")],
                            "requestBody": {
                                "required": false,
                                "content": {
                                    "application/json": {
                                        "schema": { "$ref": "#/components/schemas/Document" }
                                    }
                                }
                            },
                            "responses": {
                                "200": { "description": "Rows and column metadata" }
                            }
                        }
                    }),
                ),
                (
                    "/api/report/{name}/export".to_string(),
                    json!({
                        "get": {
                            "summary": "Export a saved query report",
                            "tags": ["report"],
                            "parameters": [
                                path_param("name"),
                                {
                                    "name": "format",
                                    "in": "query",
                                    "required": false,
                                    "schema": { "type": "string" }
                                },
                                {
                                    "name": "filters",
                                    "in": "query",
                                    "required": false,
                                    "schema": { "type": "string" }
                                }
                            ],
                            "responses": {
                                "200": { "description": "Exported file" }
                            }
                        }
                    }),
                ),
            ],
        }
    }
}

fn path_param(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "in": "path",
        "required": true,
        "schema": { "type": "string" }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Path;
    use ferris_core::User;
    use ferris_db::Stmt;
    use ferris_schema::{DoctypeMeta, FieldMeta};
    use serde_json::json;

    const FIXTURE: &str = "Report Fixture";

    fn register_all() {
        ferris_schema::doctypes::register_doctypes();
        ferris_schema::register_doctype(
            DoctypeMeta::new(FIXTURE, "test").field(FieldMeta::data("status").label("Status")),
        );
    }

    fn user(roles: &[&str]) -> User {
        User {
            id: "user-1".to_string(),
            roles: roles.iter().map(|role| (*role).to_string()).collect(),
            ..Default::default()
        }
    }

    fn system() -> User {
        user(&["System Manager"])
    }

    async fn fixture_state() -> (ApiState, String) {
        register_all();
        let state = ApiState::new(ferris_db::memory_db().await.unwrap());
        state
            .pool
            .execute_script(
                "CREATE TABLE report_fixture (name TEXT PRIMARY KEY, status TEXT NOT NULL)",
            )
            .await
            .unwrap();
        for (name, status) in [("a", "open"), ("b", "closed"), ("c", "open")] {
            state
                .pool
                .execute(&Stmt::new(
                    "INSERT INTO report_fixture (name, status) VALUES (?, ?)",
                    vec![Value::string(name), Value::string(status)],
                ))
                .await
                .unwrap();
        }

        let name = create_report(
            &state,
            "SELECT name, status FROM report_fixture \
             WHERE status = :status ORDER BY name",
        )
        .await;
        (state, name)
    }

    /// Store a saved `Report` row with `query` as its SQL.
    async fn create_report(state: &ApiState, query: &str) -> String {
        let meta = ferris_api::store::effective_meta(&state.pool, REPORT)
            .await
            .unwrap()
            .expect("Report is registered");
        let mut ctx = EngineContext::new(REPORT.to_string(), "");
        ctx.meta = Some(&meta);
        ctx.user = Some(system());
        let doc = DocumentEngine::new(state.pool.clone())
            .create(
                &ctx,
                Value::from_json(json!({
                    "report_name": "Open Items",
                    "ref_doctype": FIXTURE,
                    "report_type": "Query Report",
                    "query": query,
                })),
            )
            .await
            .expect("Report row");
        doc.get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    #[tokio::test]
    async fn a_query_report_runs_with_a_declared_filter() {
        let (state, name) = fixture_state().await;
        let result = run_report(
            &state,
            &system(),
            &name,
            &Value::from_json(json!({ "status": "open" })),
        )
        .await
        .expect("report runs");

        let columns: Vec<&str> = result
            .columns
            .iter()
            .map(|c| c.fieldname.as_str())
            .collect();
        assert_eq!(columns, vec!["name", "status"]);
        assert_eq!(result.rows.len(), 2, "only the open rows");
        assert_eq!(result.rows[0][0], Value::string("a"));
        assert_eq!(result.rows[1][0], Value::string("c"));
    }

    #[tokio::test]
    async fn an_undeclared_filter_is_a_bad_request() {
        let (state, name) = fixture_state().await;
        let error = run_report(
            &state,
            &system(),
            &name,
            &Value::from_json(json!({ "nope": "x" })),
        )
        .await
        .expect_err("an undeclared filter must not reach the binder");
        assert_eq!(
            ferris_api::router::error_response(error).status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn export_writes_the_requested_format() {
        let (state, name) = fixture_state().await;
        let csv = export_report(
            &state,
            &system(),
            &name,
            &Value::from_json(json!({ "status": "open" })),
            ExportFormat::Csv,
        )
        .await
        .expect("CSV export");
        let text = String::from_utf8(csv).unwrap();
        assert!(text.starts_with("name,status"), "{text}");
        assert!(text.contains("a,open"), "{text}");
        assert!(!text.contains("b,closed"), "{text}");

        let response = file_response(b"x".to_vec(), "Open Items", ExportFormat::Csv);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/csv; charset=utf-8"
        );
        assert_eq!(
            response.headers()[header::CONTENT_DISPOSITION],
            "attachment; filename=\"Open_Items.csv\""
        );
    }

    #[tokio::test]
    async fn an_invalid_export_format_is_a_bad_request() {
        let error = parse_export_format(Some("xlsx")).expect_err("xlsx is not supported");
        assert_eq!(
            ferris_api::router::error_response(error).status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn a_stored_report_that_mutates_is_refused() {
        let (state, _) = fixture_state().await;
        let name = create_report(&state, "DELETE FROM report_fixture").await;
        let error = run_report(&state, &system(), &name, &Value::Null)
            .await
            .expect_err("a report cannot be a DELETE");
        assert!(error.to_string().contains("read-only"), "{error}");
        assert_eq!(
            ferris_api::router::error_response(error).status(),
            StatusCode::BAD_REQUEST
        );

        let remaining = state
            .pool
            .fetch_all(&Stmt::raw("SELECT name FROM report_fixture"))
            .await
            .expect("rows still there");
        assert_eq!(remaining.len(), 3, "the mutation never ran");
    }

    #[tokio::test]
    async fn a_report_without_read_permission_is_forbidden() {
        let (state, name) = fixture_state().await;
        let error = run_report(
            &state,
            &user(&["Reader"]),
            &name,
            &Value::from_json(json!({ "status": "open" })),
        )
        .await
        .expect_err("a plain user cannot read the Report definition");
        assert!(matches!(&error, Error::Authz(_)), "{error}");
        assert_eq!(
            ferris_api::router::error_response(error).status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn only_query_reports_are_executed() {
        let doc = Value::from_json(json!({
            "name": "scripted",
            "ref_doctype": FIXTURE,
            "report_type": "Script Report",
            "query": "SELECT 1",
        }));
        let error = query_report("scripted", &doc).expect_err("script reports are not executed");
        assert_eq!(
            ferris_api::router::error_response(error).status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn the_available_reports_list_names_the_saved_report() {
        let (state, name) = fixture_state().await;
        let reports = available_reports(&state, &system()).await.unwrap();
        assert!(
            reports
                .iter()
                .any(|report| report.get("name").and_then(Value::as_str) == Some(name.as_str())),
            "{reports:?}"
        );
        assert_eq!(
            reports[0].get("ref_doctype").and_then(Value::as_str),
            Some(FIXTURE)
        );
    }
}
