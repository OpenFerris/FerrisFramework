//! List/count planning: how a plugin query maps onto the engine and which
//! predicates (`ne`, `between`) and composite sorts the host applies itself.

use super::RealHostApi;
use crate::host::{DocFilter, DocRecord, ListDocsQuery};
use anyhow::Result;
use ferris_core::Value;
use ferris_document::engine::ListQuery;
use std::cmp::Ordering;
use std::collections::HashMap;

/// Host-side range rows the engine would otherwise cap: the window pulled when
/// a plugin list needs post-processing (`ne`/`between` predicates or composite
/// ordering).
pub(super) const MAX_RANGE_ROWS: u32 = 10_000;

/// Refuse a post-processed query whose candidate set hit the scan cap.
///
/// The engine was asked for `MAX_RANGE_ROWS + 1` rows; more than the cap means
/// the set was truncated. Returning the partial count would make `count` and
/// the `list` total disagree with the data, which is exactly the silent failure
/// this host is not allowed to produce.
pub(super) fn ensure_scan_within_cap(rows: usize, cap: u32) -> Result<()> {
    if rows > cap as usize {
        return Err(anyhow::Error::new(ferris_core::Error::Validation(
            ferris_core::ValidationError::Constraint(format!(
                "query matches more than {cap} rows; add an equality filter, \
                 narrow the range, or use aggregate"
            )),
        )));
    }
    Ok(())
}

/// Page size for a `list-docs` query that omits `limit`.
const DEFAULT_LIMIT: u32 = 1000;

impl RealHostApi {
    /// Split a rich plugin list query into what the engine answers (equality
    /// map + operator filters) and what the host must apply itself (`ne`,
    /// `between`), which the engine cannot express.
    ///
    /// Every `op` token is validated here so a typo becomes a typed
    /// validation-failed error on the plugin boundary instead of silently
    /// returning an unfiltered page.
    pub(super) fn list_query_to_engine(
        query: &ListDocsQuery,
    ) -> Result<(ListQuery, Vec<DocFilter>)> {
        let mut engine = ListQuery::default();
        let mut serial = Vec::new();
        for filter in &query.filters {
            match filter.op.as_str() {
                "eq" => {
                    engine
                        .filters
                        .insert(filter.field.clone(), Value::from(filter.value.clone()));
                }
                "like" | "in" | "gt" | "gte" | "lt" | "lte" => {
                    let op = ferris_document::FilterOp::from_str(&filter.op)
                        .expect("literal operators all parse to a FilterOp");
                    engine.filter_ops.push(ferris_document::ListFilter {
                        field: filter.field.clone(),
                        op,
                        value: Value::from(filter.value.clone()),
                        or: false,
                    });
                }
                "ne" | "between" => serial.push(filter.clone()),
                other => {
                    anyhow::bail!(
                        "unsupported list-docs operator `{other}` (use eq, ne, like, in, gt, gte, lt, lte, between)"
                    )
                }
            }
        }
        Ok((engine, serial))
    }

    /// Rebuild a [`DocRecord`] from a stored document value. When the plugin
    /// asked for a projection (`fields`), `data` keeps only those fields plus
    /// the always-present system fields, while the record's `name`/`owner`/
    /// `docstatus` come from the untouched row.
    pub(super) fn doc_record(value: &Value, doctype: &str, fields: Option<&[String]>) -> DocRecord {
        let obj = value.as_object().cloned().unwrap_or_default();
        let data = fields.map_or_else(
            || serde_json::to_string(&obj).unwrap_or_default(),
            |fields| {
                let mut keep: Vec<&str> = fields.iter().map(String::as_str).collect();
                keep.extend(["name", "owner", "docstatus"]);
                let projected: HashMap<String, Value> = obj
                    .iter()
                    .filter(|(key, _)| keep.contains(&key.as_str()))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                serde_json::to_string(&projected).unwrap_or_default()
            },
        );
        DocRecord {
            name: obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            doctype: doctype.to_string(),
            owner: obj
                .get("owner")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            docstatus: obj
                .get("docstatus")
                .and_then(|v| v.as_str())
                .unwrap_or("draft")
                .to_string(),
            data,
        }
    }

    /// Rebuild a [`DocRecord`] from any stored document value.
    pub(super) fn record_from_value(doctype: &str, value: &Value) -> DocRecord {
        let obj = value.as_object().cloned().unwrap_or_default();
        DocRecord {
            name: obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            doctype: doctype.to_string(),
            owner: obj
                .get("owner")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            docstatus: obj
                .get("docstatus")
                .and_then(|v| v.as_str())
                .unwrap_or("draft")
                .to_string(),
            data: serde_json::to_string(&obj).unwrap_or_default(),
        }
    }

    /// Run a plugin list query against the engine and return the page plus the
    /// total count. With `fetch_rows == false` the page is discarded: the
    /// caller only wants the count, so no row ever crosses the boundary.
    /// Shared by `docstore_list` and `docstore_count` so both agree on
    /// authorization, predicate push-down and the `ne`/`between` fallback.
    pub(super) async fn query_documents(
        &self,
        doctype: &str,
        query: &ListDocsQuery,
        fetch_rows: bool,
        force_scan: bool,
    ) -> Result<(Vec<Value>, u64)> {
        // Authorised as the invoking user, so a plugin cannot read the whole
        // installation through the host: `docstore_query` and `docstore_get`
        // used to bypass permissions entirely (§C2).
        let (mut engine_query, serial_predicates) = Self::list_query_to_engine(query)?;
        // The host has to post-process rows (a "ne"/"between" predicate, or a
        // second sort key the engine's single order_by cannot express): the
        // whole candidate set must be visible, so no limit/offset pushdown and
        // no early projection (fields are projected below anyway, keeping the
        // system fields the engine's own `project` drops).
        let in_memory = force_scan || !serial_predicates.is_empty() || query.order_by.len() > 1;
        if in_memory {
            engine_query.order_by = None;
            // One past the cap is the sentinel that tells us the candidate set
            // did not fit: a truncated set would make `total` (and therefore
            // `count`) silently wrong, so it is refused instead.
            engine_query.limit = Some(MAX_RANGE_ROWS.saturating_add(1));
            engine_query.offset = Some(0);
        } else {
            // Missing `limit` is capped by the host, honest and bounded.
            engine_query.limit = query.limit.or(Some(DEFAULT_LIMIT));
            engine_query.offset = query.offset;
            // A single sort key is delegated so the engine can push it down.
            if let Some((field, dir)) = query.order_by.first() {
                engine_query.order_by = Some(match dir.to_lowercase().as_str() {
                    "desc" => format!("{field} desc"),
                    _ => format!("{field} asc"),
                });
            }
        }

        let (documents, engine_total) = self.engine.list(&self.ctx(doctype), &engine_query).await?;

        if !in_memory {
            // The engine owned the predicates, the page and the count.
            return Ok(if fetch_rows {
                (documents, engine_total)
            } else {
                (Vec::new(), engine_total)
            });
        }

        // Host-side predicates the engine cannot answer (`ne`, `between`),
        // evaluated over the full candidate set before paginating. The check
        // runs *before* them: the cap bounds the scan, and a set larger than it
        // was truncated, so any total derived from it would lie.
        let mut documents = documents;
        ensure_scan_within_cap(documents.len(), MAX_RANGE_ROWS)?;
        for predicate in &serial_predicates {
            documents.retain(|doc| row_matches(doc, predicate));
        }
        // Count over what survived the host-side predicates, not the engine's
        // pre-predicate total (`COUNT` must agree with `list`, R-5).
        let total = documents.len() as u64;
        if !fetch_rows {
            return Ok((Vec::new(), total));
        }
        // Composite ordering: all keys, numeric-aware like the engine.
        if !query.order_by.is_empty() {
            documents.sort_by(|a, b| compare_docs(a, b, &query.order_by));
        }
        let skip = query.offset.unwrap_or(0) as usize;
        let take = query.limit.unwrap_or(DEFAULT_LIMIT) as usize;
        documents = documents.into_iter().skip(skip).take(take).collect();
        Ok((documents, total))
    }
}

/// Cell of `field` on a stored document, coerced to its display string the
/// same way the engine's in-Rust filter does (numbers, booleans, objects).
pub(super) fn cell_value(value: &Value, field: &str) -> String {
    match value.get(field) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Numeric-aware comparison that falls back to case-insensitive text.
fn cmp_str(a: &str, b: &str) -> Ordering {
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

/// Composite sort over `(field, dir)` keys, applied in declaration order.
fn compare_docs(a: &Value, b: &Value, order_by: &[(String, String)]) -> Ordering {
    for (field, direction) in order_by {
        let desc = direction.eq_ignore_ascii_case("desc");
        let order = cmp_str(&cell_value(a, field), &cell_value(b, field));
        if order != Ordering::Equal {
            return if desc { order.reverse() } else { order };
        }
    }
    Ordering::Equal
}

/// A row predicate the engine cannot answer ("ne", "between"), evaluated over
/// display-string cells with the same comparison the engine's filters use.
fn row_matches(value: &Value, filter: &DocFilter) -> bool {
    let actual = cell_value(value, &filter.field);
    match filter.op.as_str() {
        "ne" => actual != filter.value,
        "between" => {
            let (low, high) = filter
                .value
                .split_once(',')
                .map_or(("", ""), |(low, high)| (low.trim(), high.trim()));
            // A malformed range matches nothing — the same spirit as "unparseable
            // rows are skipped" in the engine's own filter parsing.
            if low.is_empty() || high.is_empty() {
                return false;
            }
            cmp_str(low, &actual) != Ordering::Greater
                && cmp_str(&actual, high) != Ordering::Greater
        }
        _ => false,
    }
}
