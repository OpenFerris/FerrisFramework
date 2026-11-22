//! The report engine: ACL gate, read-only validation, execution and charts.

use ferris_core::{AuthzError, DatabaseError, Error, Result, ValidationError, Value};
use ferris_db::{Db, Stmt};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use crate::export::{ExportFormat, export_rows};

use super::chart::{aggregate_values, row_number, value_label};
use super::filters::{build_sql, merge_filters, read_value};
use super::types::{ChartConfig, ChartData, ChartDataset, QueryReport, ReportColumn, ReportResult};

/// Rows a report may return before the engine refuses the result.
///
/// A saved report is administrator-authored, but an unbounded `SELECT` over a
/// large table still materialises every row in this process (REP-02). The cap
/// is a ceiling, not pagination: exceeding it is an error, never a silent
/// truncation, because a partial report reads as a complete one.
pub const DEFAULT_ROW_LIMIT: u64 = 10_000;

/// Hard ceiling for [`ReportEngine::with_limits`].
///
/// A caller may ask for less (a small Desk table) or up to this ceiling, but
/// never for unlimited rows: the engine exists to bound memory, so the knob
/// that removes the bound cannot exist.
pub const MAX_ROW_LIMIT: u64 = 50_000;

/// Wall-clock budget for one report query.
///
/// Dropping the query future cancels the driver's statement and returns the
/// pooled connection, so a slow report cannot pin the pool forever.
pub const DEFAULT_QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Keywords that turn a `SELECT` into a mutation or a schema change.
///
/// `REPLACE` is deliberately absent: it is also a string function, and a
/// read-only query may legitimately call it. The first keyword check below
/// already refuses a `REPLACE INTO` statement, which cannot start with
/// `SELECT`/`WITH`.
const FORBIDDEN_KEYWORDS: &[&str] = &[
    "insert", "update", "delete", "drop", "alter", "truncate", "create", "grant", "revoke",
    "merge", "attach", "detach", "vacuum", "pragma",
];

/// Executes query reports against the database.
///
/// A16: reports may only be run through an ACL gate. The engine knows
/// nothing about users or policies, so the caller supplies a read-guard that
/// answers "can this caller read the report's source DocType?". Detailed
/// decisions live at the desk layer; `with_read_guard` is the constructor the
/// API must use. [`ReportEngine::new`] keeps a guardless engine for trusted
/// contexts only (in-crate tests, the WASM host) and is explicit about it.
pub struct ReportEngine {
    pub(super) db: Db,
    /// Verdict for `ReportEngine::run` (A16): `Some(guard)` returns `true` when
    /// the caller may read the report's `doctype`. `None` is *not* a free pass
    /// — it marks the engine as trusted-only. Filter validation below runs
    /// regardless.
    read_guard: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
    /// Row cap for one execution; see [`DEFAULT_ROW_LIMIT`].
    row_limit: u64,
    /// Deadline for one execution; see [`DEFAULT_QUERY_TIMEOUT`].
    timeout: Duration,
}

impl ReportEngine {
    /// A source DocType is what a report reads from, so an empty one is a
    /// misconfigured report, not "read everything".
    const ERR_EMPTY_DOCTYPE: &'static str = "report has no source DocType; refusing to run";

    /// Create an engine that reads without an ACL check.
    ///
    /// only for trusted contexts (framework tests, the WASM host).
    /// Anything user-facing must go through [`ReportEngine::with_read_guard`].
    pub fn new(db: Db) -> Self {
        Self::with_read_guard(db, None)
    }

    /// Create an engine whose reports only run when `read_guard(doctype)` is
    /// true. The gate answers `Permission::Read` for the report's source
    /// DocType; a report whose DocType the caller may not read is refused
    /// before any SQL is executed.
    pub fn with_read_guard(
        db: Db,
        read_guard: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
    ) -> Self {
        Self {
            db,
            read_guard,
            row_limit: DEFAULT_ROW_LIMIT,
            timeout: DEFAULT_QUERY_TIMEOUT,
        }
    }

    /// Lower (or cap) the rows and the wall-clock budget of one execution.
    ///
    /// Both values are clamped rather than rejected: a request may ask for a
    /// smaller report, but no caller can disable the bound the engine exists to
    /// enforce. A zero row limit is treated as one row; a zero duration as one
    /// millisecond.
    #[must_use]
    pub fn with_limits(mut self, row_limit: u64, timeout: Duration) -> Self {
        self.row_limit = row_limit.clamp(1, MAX_ROW_LIMIT);
        self.timeout = timeout.max(Duration::from_millis(1));
        self
    }

    /// Run a report, applying filter defaults and binding every value.
    pub async fn run(&self, report: &QueryReport, filters: &Value) -> Result<ReportResult> {
        self.authorize(report)?;
        // The stored SQL is untrusted even though only an administrator writes
        // the row: a "report" that mutates data would bypass hooks, versions and
        // permissions (REP-01). Both checks run before any filter is bound, so a
        // refused report never reaches the database.
        validate_read_only(&report.query)?;
        let merged = merge_filters(report, filters)?;
        let (sql, params) = build_sql(&report.query, &merged)?;

        let stmt = Stmt::new(limit_query(&sql, self.row_limit), params);
        let rows = self.fetch_bounded(&stmt).await?;
        // The SQL carries `LIMIT row_limit + 1`; an extra row means the report
        // would have been truncated. Failing is deliberate: a capped report that
        // looks complete is worse than an error telling the author to narrow it.
        if u64::try_from(rows.len()).unwrap_or(u64::MAX) > self.row_limit {
            return Err(Error::Validation(ValidationError::Invalid(format!(
                "report '{}' returned more than {} rows; add a filter or narrow the query",
                report.name, self.row_limit
            ))));
        }

        // Columns come from the report definition, or are derived from the
        // result set when the report does not declare them.
        let columns = if report.columns.is_empty() {
            rows.first()
                .map(|row| {
                    row.columns()
                        .into_iter()
                        .map(|c| ReportColumn::new(c, c, "Data"))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            report.columns.clone()
        };

        let data = rows
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .map(|c| read_value(row, &c.fieldname))
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(ReportResult {
            columns,
            rows: data,
        })
    }

    /// Fetch every row, unless the deadline expires first.
    ///
    /// `fetch_all` has no cancellation of its own; dropping the returned future
    /// is what makes the driver abandon the statement, so the timeout returns a
    /// typed error instead of leaving the connection busy.
    async fn fetch_bounded(&self, stmt: &Stmt) -> Result<Vec<ferris_db::Row>> {
        let timeout = self.timeout;
        match with_deadline(timeout, self.db.fetch_all(stmt)).await {
            Some(rows) => rows,
            None => Err(Error::Database(DatabaseError::Query(format!(
                "report query exceeded its {} ms deadline",
                timeout.as_millis()
            )))),
        }
    }

    /// A16: a report must name a source DocType and the caller proves it may
    /// read it. Runs before any SQL is built, so a denied (or misconfigured)
    /// report never touches the database.
    fn authorize(&self, report: &QueryReport) -> Result<()> {
        if report.doctype.trim().is_empty() {
            return Err(Error::Validation(ValidationError::Required(
                Self::ERR_EMPTY_DOCTYPE.to_string(),
            )));
        }
        if let Some(guard) = &self.read_guard {
            if !guard(&report.doctype) {
                return Err(Error::Authz(AuthzError::PermissionDenied {
                    action: "read".to_string(),
                    resource: report.doctype.clone(),
                }));
            }
        }
        Ok(())
    }

    /// Run a report and serialise it.
    ///
    /// The export path deliberately goes through [`Self::run`]: the row cap,
    /// the deadline and the read-only check therefore apply to a CSV/JSON
    /// download exactly as they do to the Desk table, which is the path a
    /// larger result would otherwise take to exhaust memory (REP-02).
    pub async fn export(
        &self,
        report: &QueryReport,
        filters: &Value,
        format: ExportFormat,
    ) -> Result<Vec<u8>> {
        let result = self.run(report, filters).await?;
        export_rows(&result, format)
    }

    /// Turn a report result into chart-ready data.
    ///
    /// Rows are grouped by `chart.x_field` (first-seen order) and each
    /// `y_fields` entry is aggregated per group. With no `y_fields`, the chart
    /// counts rows per label.
    pub fn chart(&self, result: &ReportResult, chart: &ChartConfig) -> Result<ChartData> {
        let column_index = |field: &str| {
            result
                .columns
                .iter()
                .position(|c| c.fieldname == field)
                .ok_or_else(|| {
                    Error::Validation(ValidationError::Invalid(format!(
                        "Chart field '{field}' is not in the report columns"
                    )))
                })
        };

        let x_index = column_index(&chart.x_field)?;
        let aggregate = chart.aggregate.unwrap_or_default();

        // Preserve the order in which labels first appear.
        let mut labels: Vec<String> = Vec::new();
        let mut groups: HashMap<String, Vec<&Vec<Value>>> = HashMap::new();
        for row in &result.rows {
            let label = value_label(row.get(x_index).unwrap_or(&Value::Null));
            if !groups.contains_key(&label) {
                labels.push(label.clone());
            }
            groups.entry(label).or_default().push(row);
        }

        let mut datasets = Vec::new();
        if chart.y_fields.is_empty() {
            let values = labels
                .iter()
                .map(|label| {
                    groups
                        .get(label)
                        .map(|rows| rows.len() as f64)
                        .unwrap_or(0.0)
                })
                .collect();
            datasets.push(ChartDataset {
                name: "count".to_string(),
                values,
            });
        } else {
            let y_indices: Vec<usize> = chart
                .y_fields
                .iter()
                .map(|field| column_index(field))
                .collect::<Result<Vec<_>>>()?;
            for (field, index) in chart.y_fields.iter().zip(y_indices) {
                let values = labels
                    .iter()
                    .map(|label| {
                        let numbers = groups
                            .get(label)
                            .map(|rows| {
                                rows.iter()
                                    .map(|row| row_number(row.get(index)))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        aggregate_values(numbers, aggregate)
                    })
                    .collect();
                datasets.push(ChartDataset {
                    name: field.clone(),
                    values,
                });
            }
        }

        Ok(ChartData {
            chart_type: chart.chart_type,
            labels,
            datasets,
        })
    }

    /// Convenience: run the report and build its configured chart.
    pub async fn run_chart(&self, report: &QueryReport, filters: &Value) -> Result<ChartData> {
        let config = report.chart.as_ref().ok_or_else(|| {
            Error::Validation(ValidationError::Invalid(format!(
                "Report '{}' has no chart configuration",
                report.name
            )))
        })?;
        let result = self.run(report, filters).await?;
        self.chart(&result, config)
    }
}

/// Reject anything but a single read-only statement.
///
/// A saved report is administrator-authored, but the SQL is still data: nothing
/// stopped a row from carrying `DELETE`, and a "read-only" report that mutates
/// rows bypasses hooks, versions and permissions (REP-01). This is a lexer, not
/// a SQL parser: it masks string literals, quoted identifiers and comments,
/// then requires the first word to be `SELECT` or `WITH` and refuses a mutation
/// keyword in the remainder.
///
/// Two consequences are deliberate:
///
/// * a column named exactly like a keyword (`update`) is refused, while
///   `updated_at` or a keyword inside a literal/comment is not — the rule is
///   "fail closed", and report authors can quote a name;
/// * PostgreSQL dollar-quoted strings (`$$…$$`) are not understood (REP-08),
///   so a keyword inside one is refused rather than silently allowed.
fn validate_read_only(query: &str) -> Result<()> {
    let masked = mask_sql(query);
    let mut statements = masked.split(';');
    let first = statements.next().unwrap_or_default().trim();
    if statements.any(|rest| !rest.trim().is_empty()) {
        return Err(Error::Validation(ValidationError::Invalid(
            "a report must be a single SQL statement".to_string(),
        )));
    }
    let words: Vec<String> = sql_words(first);
    let Some(head) = words.first() else {
        return Err(Error::Validation(ValidationError::Invalid(
            "report SQL is empty".to_string(),
        )));
    };
    if !matches!(head.as_str(), "select" | "with") {
        return Err(Error::Validation(ValidationError::Invalid(format!(
            "a report must be read-only and start with SELECT or WITH, not `{head}`"
        ))));
    }
    if let Some(keyword) = words
        .iter()
        .find(|word| FORBIDDEN_KEYWORDS.contains(&word.as_str()))
    {
        return Err(Error::Validation(ValidationError::Invalid(format!(
            "a report must be read-only: `{keyword}` is not allowed"
        ))));
    }
    Ok(())
}

/// Replace literals, quoted identifiers and comments with spaces.
///
/// Newlines are preserved so a line comment stays terminated. The scanner is
/// char-based so a multi-byte literal cannot be split, and it is total: an
/// unterminated quote or comment simply masks the rest of the input.
fn mask_sql(query: &str) -> String {
    #[derive(Clone, Copy)]
    enum State {
        Normal,
        Single,
        Double,
        Backtick,
        LineComment,
        BlockComment,
    }

    let mut out = String::with_capacity(query.len());
    let mut state = State::Normal;
    let mut chars = query.chars().peekable();
    while let Some(c) = chars.next() {
        match state {
            State::Normal => match c {
                '\'' => {
                    state = State::Single;
                    out.push(' ');
                }
                '"' => {
                    state = State::Double;
                    out.push(' ');
                }
                '`' => {
                    state = State::Backtick;
                    out.push(' ');
                }
                '-' if chars.peek() == Some(&'-') => {
                    chars.next();
                    state = State::LineComment;
                    out.push(' ');
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    state = State::BlockComment;
                    out.push(' ');
                }
                _ => out.push(c),
            },
            State::Single | State::Double | State::Backtick => {
                let closing = match state {
                    State::Single => '\'',
                    State::Double => '"',
                    _ => '`',
                };
                if c == closing {
                    // `''` inside a string literal is an escaped quote, not the
                    // end of it; a backtick identifier has no such escape.
                    if !matches!(state, State::Backtick) && chars.peek() == Some(&closing) {
                        chars.next();
                    } else {
                        state = State::Normal;
                    }
                }
                out.push(' ');
            }
            State::LineComment => {
                out.push(if c == '\n' { '\n' } else { ' ' });
                if c == '\n' {
                    state = State::Normal;
                }
            }
            State::BlockComment => {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    state = State::Normal;
                }
                out.push(' ');
            }
        }
    }
    out
}

/// Lowercase alphanumeric/underscore words of masked SQL.
fn sql_words(masked: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for c in masked.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            current.push(c.to_ascii_lowercase());
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Wrap `sql` in a row cap.
///
/// Wrapping instead of appending keeps a query that already ends in `LIMIT`
/// valid, and it works for `WITH` queries and every supported dialect (the
/// subquery alias is mandatory in MySQL). ORDER BY stays inside the subquery,
/// which SQLite/PostgreSQL/MySQL preserve for a plain outer scan.
fn limit_query(sql: &str, row_limit: u64) -> String {
    let inner = sql.trim().trim_end_matches(';').trim_end();
    // One row over the cap, so `run` can tell "exactly at the cap" from
    // "truncated" without a second query.
    let probe = row_limit.saturating_add(1);
    format!("SELECT * FROM ({inner}) AS ferris_report_rows LIMIT {probe}")
}

/// Run `future`, or give up after `limit`.
///
/// The timer is a plain thread because this crate declares no async-runtime
/// dependency: the engine has to be usable by a process that only links
/// `ferris-db`, and adding `tokio` here for one `timeout` would make the query
/// engine drag a runtime into every consumer. The thread is detached and exits
/// at the deadline at the latest; dropping `future` is what cancels the
/// driver's statement.
async fn with_deadline<T>(limit: Duration, future: impl Future<Output = T>) -> Option<T> {
    let mut future = std::pin::pin!(future);
    let mut deadline = std::pin::pin!(Deadline::after(limit));
    std::future::poll_fn(|cx| {
        if let Poll::Ready(value) = future.as_mut().poll(cx) {
            return Poll::Ready(Some(value));
        }
        if deadline.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

/// Wake its task once a deadline passes.
struct Deadline {
    state: Arc<Mutex<DeadlineState>>,
}

struct DeadlineState {
    expired: bool,
    waker: Option<Waker>,
}

impl Deadline {
    fn after(limit: Duration) -> Self {
        let state = Arc::new(Mutex::new(DeadlineState {
            expired: false,
            waker: None,
        }));
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            std::thread::sleep(limit);
            // A poisoned lock means a previous poll panicked; waking nobody is
            // safe because the future is already gone in that case.
            let Ok(mut guard) = shared.lock() else {
                return;
            };
            guard.expired = true;
            if let Some(waker) = guard.waker.take() {
                waker.wake();
            }
        });
        Self { state }
    }
}

impl Future for Deadline {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let Ok(mut guard) = self.state.lock() else {
            // Fail closed: an unusable timer must not turn into "no deadline".
            return Poll::Ready(());
        };
        if guard.expired {
            Poll::Ready(())
        } else {
            guard.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_db::memory_db;

    fn report(query: &str) -> QueryReport {
        QueryReport {
            name: "Fixture".to_string(),
            doctype: "Note".to_string(),
            query: query.to_string(),
            columns: Vec::new(),
            filters: Vec::new(),
            chart: None,
        }
    }

    #[test]
    fn mutations_are_not_reports() {
        for query in [
            "DELETE FROM tab_note",
            "UPDATE tab_note SET title = 'x'",
            "INSERT INTO tab_note (name) VALUES ('x')",
            "DROP TABLE tab_note",
            "ALTER TABLE tab_note ADD COLUMN x TEXT",
            "TRUNCATE tab_note",
            "WITH doomed AS (SELECT name FROM tab_note) DELETE FROM tab_note",
            "SELECT 1; SELECT 2",
            "SELECT 1; DELETE FROM tab_note",
            "EXPLAIN DELETE FROM tab_note",
        ] {
            assert!(
                validate_read_only(query).is_err(),
                "`{query}` must not be accepted as a report"
            );
        }
    }

    #[test]
    fn a_read_only_select_is_accepted() {
        for query in [
            "SELECT name, title FROM tab_note",
            "  select name from tab_note;  ",
            "WITH recent AS (SELECT name FROM tab_note) SELECT * FROM recent",
            "SELECT replace(title, 'a', 'b') FROM tab_note",
            "SELECT updated_at FROM tab_note",
        ] {
            assert!(validate_read_only(query).is_ok(), "`{query}` is read-only");
        }
    }

    #[test]
    fn a_keyword_inside_a_literal_or_comment_is_data() {
        for query in [
            "SELECT name FROM tab_note WHERE title = 'delete me'",
            "SELECT name FROM tab_note WHERE title = 'drop; table'",
            "SELECT name FROM tab_note -- delete everything\nWHERE name = 'x'",
            "SELECT name /* truncate; delete */ FROM tab_note",
            "SELECT \"delete\" FROM tab_note",
        ] {
            assert!(
                validate_read_only(query).is_ok(),
                "`{query}` has no mutation keyword outside a literal"
            );
        }
    }

    #[tokio::test]
    async fn a_report_over_its_row_cap_is_refused_instead_of_truncated() {
        let db = memory_db().await.expect("db");
        db.execute_script("CREATE TABLE note (name TEXT PRIMARY KEY)")
            .await
            .expect("table");
        for name in ["a", "b", "c"] {
            db.execute(&Stmt::new(
                "INSERT INTO note (name) VALUES (?)",
                vec![Value::string(name)],
            ))
            .await
            .expect("row");
        }

        let engine = ReportEngine::new(db).with_limits(2, DEFAULT_QUERY_TIMEOUT);
        let error = engine
            .run(&report("SELECT name FROM note ORDER BY name"), &Value::Null)
            .await
            .expect_err("three rows do not fit a two-row cap");
        assert!(error.to_string().contains("more than 2 rows"), "{error}");
    }

    #[tokio::test]
    async fn a_mutating_report_never_reaches_the_database() {
        let db = memory_db().await.expect("db");
        db.execute_script("CREATE TABLE note (name TEXT PRIMARY KEY)")
            .await
            .expect("table");
        let engine = ReportEngine::new(db.clone());

        let error = engine
            .run(&report("DELETE FROM note"), &Value::Null)
            .await
            .expect_err("a mutation is refused");
        assert!(error.to_string().contains("read-only"), "{error}");

        let rows = db
            .fetch_all(&Stmt::raw("SELECT name FROM note"))
            .await
            .expect("query still works");
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn an_endless_query_hits_the_deadline() {
        // The future never resolves, so the assertion does not race the query:
        // it proves the deadline path resolves the wrapper at all.
        let outcome: Option<()> =
            with_deadline(Duration::from_millis(5), std::future::pending()).await;
        assert!(outcome.is_none(), "a pending query must time out");
    }
}
