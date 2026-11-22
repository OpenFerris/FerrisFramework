//! Filter validation, the `:name` SQL binder and result-cell decoding.

use ferris_core::{Error, Result, ValidationError, Value};
use ferris_db::Row;

use super::types::{QueryReport, ReportFilter};

/// Fill missing filters from the report's defaults, rejecting missing required
/// filters and request-supplied keys the report never declared.
///
/// A16: `merge_filters` used to accept any object and silently pass
/// arbitrary keys on to the binder. Nothing interpolates SQL, but an
/// undeclared key is still a value the report author never exposed, so it is
/// refused the same way Frappe's `validate_filters_permissions` refuses a
/// caller-supplied filter that is not part of the report.
pub(super) fn merge_filters(report: &QueryReport, filters: &Value) -> Result<Value> {
    let mut map = match filters {
        Value::Object(map) => map.clone(),
        Value::Null => ferris_core::Map::new(),
        _ => {
            return Err(Error::Validation(ValidationError::Invalid(
                "Report filters must be an object".to_string(),
            )));
        }
    };

    let declared: Vec<&str> = report
        .filters
        .iter()
        .map(|f| f.fieldname.as_str())
        .collect();
    for key in map.keys() {
        if !declared.contains(&key.as_str()) {
            return Err(Error::Validation(ValidationError::Invalid(format!(
                "Filter '{key}' is not declared by report '{}'",
                report.name
            ))));
        }
    }

    for filter in &report.filters {
        let present = map
            .get(&filter.fieldname)
            .map(|v| !v.is_null())
            .unwrap_or(false);
        if !present {
            if let Some(default) = &filter.default {
                map.insert(filter.fieldname.clone(), default.clone());
            } else if filter.reqd {
                return Err(Error::Validation(ValidationError::Required(
                    filter.fieldname.clone(),
                )));
            }
        }
    }

    Ok(Value::Object(map))
}

/// Replace `:name` placeholders with `?` and collect bound values.
///
/// `:name` is only treated as a placeholder when it starts with a letter or
/// underscore, so time literals (`'12:30'`) and Postgres casts (`::text`) are
/// left untouched.
fn scan_sql(query: &str, filters: Option<&Value>) -> Result<(String, Vec<Value>, Vec<String>)> {
    // The scanner has to know when a `:` is data: a quoted literal (`'a:b'`),
    // a line comment (`-- x:y`) and a block comment (`/* x:y */`) must keep
    // their text, or a legitimate report fails with "missing value for b"
    // (RPT5). Scanning by `char` instead of by byte also stops multi-byte text
    // from being mangled into Latin-1 lookalikes.
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Normal,
        Single,
        Double,
        LineComment,
        BlockComment,
    }

    let mut sql = String::with_capacity(query.len());
    let mut params = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut state = State::Normal;
    let mut i = 0usize;

    while i < query.len() {
        let Some(c) = query[i..].chars().next() else {
            break;
        };
        let next = query[i + c.len_utf8()..].chars().next();
        match state {
            State::Normal => match c {
                '\'' => {
                    sql.push(c);
                    state = State::Single;
                    i += c.len_utf8();
                }
                '"' => {
                    sql.push(c);
                    state = State::Double;
                    i += c.len_utf8();
                }
                '-' if next == Some('-') => {
                    sql.push_str("--");
                    state = State::LineComment;
                    i += 2;
                }
                '/' if next == Some('*') => {
                    sql.push_str("/*");
                    state = State::BlockComment;
                    i += 2;
                }
                ':' if next == Some(':') => {
                    sql.push_str("::");
                    i += 2;
                }
                ':' => {
                    let start = i + 1;
                    let bytes = query.as_bytes();
                    if start < bytes.len()
                        && (bytes[start].is_ascii_alphabetic() || bytes[start] == b'_')
                    {
                        let mut j = start;
                        while j < bytes.len()
                            && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
                        {
                            j += 1;
                        }
                        let name = &query[start..j];
                        if !names.iter().any(|seen| seen == name) {
                            names.push(name.to_string());
                        }
                        // With `filters = None` the scanner only collects the
                        // declared names (used to build a report's filter
                        // metadata); it does not require a value for each.
                        if let Some(filters) = filters {
                            let value =
                                filters.get(name).filter(|v| !v.is_null()).ok_or_else(|| {
                                    Error::Validation(ValidationError::Required(name.to_string()))
                                })?;
                            params.push(value.clone());
                        }
                        sql.push('?');
                        i = j;
                    } else {
                        sql.push(c);
                        i += c.len_utf8();
                    }
                }
                _ => {
                    sql.push(c);
                    i += c.len_utf8();
                }
            },
            State::Single => {
                sql.push(c);
                if c == '\'' {
                    state = State::Normal;
                }
                i += c.len_utf8();
            }
            State::Double => {
                sql.push(c);
                if c == '"' {
                    state = State::Normal;
                }
                i += c.len_utf8();
            }
            State::LineComment => {
                sql.push(c);
                if c == '\n' {
                    state = State::Normal;
                }
                i += c.len_utf8();
            }
            State::BlockComment => {
                sql.push(c);
                if c == '*' && next == Some('/') {
                    sql.push('/');
                    state = State::Normal;
                    i += 2;
                } else {
                    i += c.len_utf8();
                }
            }
        }
    }

    Ok((sql, params, names))
}

/// Bind `filters` into `query` and return the executable SQL plus its params.
pub(super) fn build_sql(query: &str, filters: &Value) -> Result<(String, Vec<Value>)> {
    let (sql, params, _) = scan_sql(query, Some(filters))?;
    Ok((sql, params))
}

/// The `:name` placeholders a report query declares, in first-seen order.
///
/// The API uses this to build [`QueryReport::filters`] from the stored SQL, so
/// a client can pass `{ "status": "open" }` without a separate filter
/// declaration in the `Report` DocType. Placeholders inside string literals and
/// comments are data, not filters (the same scanner `build_sql` uses).
#[must_use]
pub fn declared_filters(query: &str) -> Vec<ReportFilter> {
    scan_sql(query, None)
        .map(|(_, _, names)| {
            names
                .into_iter()
                .map(|name| ReportFilter::new(name.clone(), name, "Data"))
                .collect()
        })
        .unwrap_or_default()
}

/// One report cell, decoded through [`Row`] so the dialect differences live in
/// one place. A `NULL`, or a column the result set does not carry, is an empty
/// report cell and reads as [`Value::Null`].
pub(super) fn read_value(row: &Row, field: &str) -> Result<Value> {
    // A declared column the SQL does not return used to become `Null` in every
    // row, which reads as "all values are empty" instead of "the report is
    // broken" (RPT6).
    if !row.has(field) {
        return Err(Error::Validation(ValidationError::Invalid(format!(
            "report column '{field}' is not present in the query result"
        ))));
    }
    Ok(row
        .opt_str(field)
        .map(Value::from)
        .or_else(|| row.opt_i64(field).map(Value::from))
        .or_else(|| row.opt_f64(field).map(Value::from))
        .or_else(|| row.opt_bool(field).map(Value::from))
        .unwrap_or(Value::Null))
}
