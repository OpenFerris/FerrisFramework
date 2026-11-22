use super::filters::build_sql;
use super::*;

use crate::export::ExportFormat;
use ferris_core::Value;
use ferris_db::{Db, Stmt, sql::Column};
use std::sync::Arc;

async fn db() -> Db {
    ferris_db::memory_db().await.unwrap()
}

async fn seed(db: &Db) {
    db.ensure_table(
        "tab_todo",
        &[
            Column::new("name", "VARCHAR(255)").not_null().primary_key(),
            Column::new("title", "TEXT"),
            Column::new("status", "VARCHAR(32)"),
        ],
    )
    .await
    .unwrap();
    for (name, title, status) in [
        ("TODO-1", "Buy milk", "open"),
        ("TODO-2", "Write docs", "closed"),
        ("TODO-3", "Ship it", "open"),
    ] {
        db.execute(&Stmt::new(
            "INSERT INTO tab_todo (name, title, status) VALUES (?, ?, ?)",
            vec![Value::from(name), Value::from(title), Value::from(status)],
        ))
        .await
        .unwrap();
    }
}

fn report() -> QueryReport {
    QueryReport {
        name: "Open Todos".to_string(),
        doctype: "ToDo".to_string(),
        query: "SELECT name, title FROM tab_todo WHERE status = :status ORDER BY name".to_string(),
        columns: vec![
            ReportColumn::new("name", "ID", "Data"),
            ReportColumn::new("title", "Title", "Data"),
        ],
        filters: vec![ReportFilter::new("status", "Status", "Data").required()],
        chart: None,
    }
}

#[tokio::test]
async fn test_run_query_report_binds_filters() {
    let db = db().await;
    seed(&db).await;
    let engine = ReportEngine::new(db);

    let result = engine
        .run(&report(), &ferris_core::value!({ "status": "open" }))
        .await
        .unwrap();
    assert_eq!(result.total(), 2);
    assert_eq!(result.rows[0][0].as_str(), Some("TODO-1"));
    assert_eq!(result.rows[1][1].as_str(), Some("Ship it"));
}

#[tokio::test]
async fn test_missing_required_filter_is_rejected() {
    let db = db().await;
    seed(&db).await;
    let engine = ReportEngine::new(db);
    assert!(engine.run(&report(), &Value::Null).await.is_err());
}

/// A16: the read-guard is evaluated before any SQL is built or run — a
/// report whose source DocType the caller may not read is refused.
#[tokio::test]
async fn test_report_guard_denies_unreadable_doctype() {
    let db = db().await;
    seed(&db).await;
    let deny_todo = Arc::new(|doctype: &str| doctype != "ToDo");
    let engine = ReportEngine::with_read_guard(db, Some(deny_todo));

    let err = engine
        .run(&report(), &ferris_core::value!({ "status": "open" }))
        .await
        .unwrap_err();
    let text = err.to_string();
    assert!(text.contains("permission denied"), "{text}");
}

/// A16: the same report is fine once the guard admits its DocType.
#[tokio::test]
async fn test_report_guard_allows_readable_doctype() {
    let db = db().await;
    seed(&db).await;
    let allow_todo = Arc::new(|doctype: &str| doctype == "ToDo");
    let engine = ReportEngine::with_read_guard(db, Some(allow_todo));

    let result = engine
        .run(&report(), &ferris_core::value!({ "status": "open" }))
        .await
        .unwrap();
    assert_eq!(result.total(), 2);
}

/// A16: a report without a source DocType is a misconfiguration, never a
/// "read everything" licence.
#[tokio::test]
async fn test_report_without_doctype_is_refused() {
    let db = db().await;
    seed(&db).await;
    let mut r = report();
    r.doctype = "  ".to_string();

    let err = ReportEngine::new(db)
        .run(&r, &ferris_core::value!({ "status": "open" }))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no source DocType"), "{err}");
}

/// A16: request-supplied filter keys the report never declared are
/// refused, matching Frappe's `validate_filters_permissions`.
#[tokio::test]
async fn test_undeclared_filter_keys_are_rejected() {
    let db = db().await;
    seed(&db).await;
    let engine = ReportEngine::new(db);

    let err = engine
        .run(
            &report(),
            &ferris_core::value!({ "status": "open", "owner": "mallory" }),
        )
        .await
        .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("owner") && text.contains("not declared"),
        "{text}"
    );
}

#[tokio::test]
async fn test_filter_value_is_not_interpolated() {
    let db = db().await;
    seed(&db).await;
    let engine = ReportEngine::new(db);

    // A classic injection payload is treated as a literal value.
    let result = engine
        .run(
            &report(),
            &ferris_core::value!({ "status": "open' OR '1'='1" }),
        )
        .await
        .unwrap();
    assert_eq!(result.total(), 0);
}

#[tokio::test]
async fn test_build_sql_ignores_casts_and_time() {
    let (sql, params) = build_sql(
        "SELECT now()::text AS t, '12:30' AS time, :status AS s",
        &ferris_core::value!({ "status": "open" }),
    )
    .unwrap();
    assert!(sql.contains("::text"));
    assert!(sql.contains("'12:30'"));
    assert!(sql.contains("? AS s"));
    assert_eq!(params.len(), 1);
}

/// RPT5: a `:name` inside a quoted literal or a comment is data, not a
/// placeholder; a real placeholder is still substituted.
#[tokio::test]
async fn test_build_sql_ignores_literals_and_comments() {
    let (sql, params) = build_sql(
        "SELECT 'a:b' AS lit -- c:d\n/* e:f */ FROM t WHERE x = :x",
        &ferris_core::value!({ "x": 1 }),
    )
    .unwrap();
    assert!(sql.contains("'a:b'"), "literal was rewritten: {sql}");
    assert!(sql.contains("-- c:d"));
    assert!(sql.contains("/* e:f */"));
    assert!(sql.contains("x = ?"));
    assert_eq!(params.len(), 1);
}

/// RPT6: a declared column the query does not return is an error, not a
/// column of nulls.
#[tokio::test]
async fn a_declared_column_missing_from_the_result_is_an_error() {
    let db = db().await;
    seed(&db).await;
    let report = QueryReport {
        name: "broken".to_string(),
        doctype: "ToDo".to_string(),
        query: "SELECT name FROM tab_todo".to_string(),
        columns: vec![ReportColumn::new("nope", "Nope", "Data")],
        filters: Vec::new(),
        chart: None,
    };
    let engine = ReportEngine::new(db);
    let error = engine
        .run(&report, &Value::Object(Default::default()))
        .await
        .expect_err("a missing column must be reported");
    assert!(
        error.to_string().contains("nope"),
        "error should name the column: {error}"
    );
}

#[tokio::test]
async fn test_export_and_prepare_roundtrip() {
    let db = db().await;
    seed(&db).await;
    let engine = ReportEngine::new(db);
    engine.init().await.unwrap();

    let filters = ferris_core::value!({ "status": "open" });
    let csv = engine
        .export(&report(), &filters, ExportFormat::Csv)
        .await
        .unwrap();
    let csv = String::from_utf8(csv).unwrap();
    assert!(csv.starts_with("ID,Title\n"));
    assert!(csv.contains("TODO-3,Ship it"));

    let prepared = engine.prepare(&report(), &filters).await.unwrap();
    assert_eq!(prepared.status, PreparedReportStatus::Completed);
    let loaded = engine.get_prepared(&prepared.id).await.unwrap().unwrap();
    assert_eq!(loaded.report, "Open Todos");
    assert_eq!(loaded.result.unwrap().total(), 2);
}

#[tokio::test]
async fn test_chart_aggregation() {
    let db = db().await;
    seed(&db).await;
    let engine = ReportEngine::new(db);

    let report = QueryReport {
        name: "By status".to_string(),
        doctype: "ToDo".to_string(),
        query: "SELECT status, 1 AS one FROM tab_todo ORDER BY name".to_string(),
        columns: vec![
            ReportColumn::new("status", "Status", "Data"),
            ReportColumn::new("one", "One", "Int"),
        ],
        filters: vec![],
        chart: Some(ChartConfig {
            chart_type: ChartType::Bar,
            x_field: "status".to_string(),
            y_fields: vec!["one".to_string()],
            aggregate: Some(ChartAggregate::Sum),
        }),
    };

    let result = engine.run(&report, &Value::Null).await.unwrap();
    let chart = engine
        .chart(&result, report.chart.as_ref().unwrap())
        .unwrap();
    assert_eq!(chart.labels, vec!["open".to_string(), "closed".to_string()]);
    assert_eq!(chart.datasets[0].name, "one");
    assert_eq!(chart.datasets[0].values, vec![2.0, 1.0]);

    // With no y_fields the chart counts rows per label.
    let count_chart = engine
        .chart(
            &result,
            &ChartConfig {
                chart_type: ChartType::Pie,
                x_field: "status".to_string(),
                y_fields: vec![],
                aggregate: None,
            },
        )
        .unwrap();
    assert_eq!(count_chart.datasets[0].name, "count");
    assert_eq!(count_chart.datasets[0].values, vec![2.0, 1.0]);

    // Convenience wrapper uses the report's own chart config.
    let chart = engine.run_chart(&report, &Value::Null).await.unwrap();
    assert_eq!(chart.chart_type, ChartType::Bar);

    // Unknown fields are rejected.
    assert!(
        engine
            .chart(
                &result,
                &ChartConfig {
                    chart_type: ChartType::Line,
                    x_field: "missing".to_string(),
                    y_fields: vec![],
                    aggregate: None,
                },
            )
            .is_err()
    );
}

#[test]
fn declared_filters_finds_placeholders_and_ignores_literals_and_comments() {
    let filters = declared_filters(
        "SELECT * FROM tab_x WHERE status = :status AND label = 'a:b' \
         AND created > :since -- :ignored\n /* :also_ignored */",
    );
    let names: Vec<&str> = filters.iter().map(|f| f.fieldname.as_str()).collect();
    assert_eq!(names, vec!["status", "since"]);
}

#[test]
fn declared_filters_deduplicates_and_handles_no_placeholders() {
    let filters = declared_filters("SELECT :a, :a, :b FROM tab_x");
    let names: Vec<&str> = filters.iter().map(|f| f.fieldname.as_str()).collect();
    assert_eq!(names, vec!["a", "b"]);
    assert!(declared_filters("SELECT 1").is_empty());
}
