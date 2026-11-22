//! Auto repeat: periodically create a fresh copy of a reference document.
//!
//! The pure scheduling core ([`advance`], [`is_due`]) is unit-tested without a
//! database; [`run_due`] performs the work: for every due, enabled `Auto Repeat`
//! row it clones the reference document (fields minus the engine-owned system
//! fields, so the copy gets a fresh name/owner/audit trail) and rolls
//! `next_schedule_date` forward. `end_date` rows stop being due after that
//! date — the row stays enabled so an operator sees why it went quiet.
//!
//! The reference doctype is read through the service user; a clone of a
//! DocType the Integrations principal cannot read is skipped (and logged — an
//! Auto Repeat against a permission-protected type needs an operator's review).

use std::collections::HashMap;

use chrono::{Datelike, Days, NaiveDate};
use ferris_core::{Result, Value};

use super::names::AUTO_REPEAT;
use super::state::IntegrationsContext;
use super::store;

/// Fields the engine owns and a clone must not carry over (the copy is inserted
/// as a new document with its own audit trail).
const SYSTEM_FIELDS: &[&str] = &[
    "name",
    "owner",
    "creation",
    "modified",
    "modified_by",
    "doctype",
    "idx",
    "docstatus",
    "_user_tags",
    "_comments",
    "_assign",
    "_liked_by",
    "_seen",
    "_names_seen",
];

fn str_field(doc: &Value, key: &str) -> String {
    doc.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn bool_field(doc: &Value, key: &str, default: bool) -> bool {
    doc.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn date_field(doc: &Value, key: &str) -> Option<NaiveDate> {
    doc.get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
}

/// Advance `on` by one `frequency` period.
///
/// `repeat_on_day` pins the day of the month for `Monthly`/`Quarterly`/
/// `Yearly` (clamped to the month's last day); `None` keeps the current
/// day-of-month. `Daily`/`Weekly` ignore it.
#[must_use]
pub fn advance(frequency: &str, on: NaiveDate, repeat_on_day: Option<i64>) -> NaiveDate {
    let day = repeat_on_day
        .and_then(|d| u32::try_from(d).ok())
        .filter(|d| *d >= 1);
    match frequency.trim().to_lowercase().as_str() {
        "weekly" => on + Days::new(7),
        "monthly" => advance_months(on, 1, day),
        "quarterly" => advance_months(on, 3, day),
        "yearly" => advance_months(on, 12, day),
        // Daily and unknown frequencies: one day, so a typo cannot silently
        // stop recurring documents.
        _ => on.succ_opt().unwrap_or(on),
    }
}

fn advance_months(on: NaiveDate, months: i64, repeat_on_day: Option<u32>) -> NaiveDate {
    let months_total = i64::from(on.year()) * 12 + i64::from(on.month0()) + months;
    let year = i32::try_from(months_total.div_euclid(12)).unwrap_or_else(|_| on.year());
    let month = u32::try_from(months_total.rem_euclid(12) + 1).unwrap_or(1);
    let days_in_month =
        NaiveDate::from_ymd_opt(year, month, 1).map_or(28, |first| first.num_days_in_month());
    let target_day = repeat_on_day
        .filter(|d| *d >= 1 && *d <= u32::from(days_in_month))
        .unwrap_or_else(|| on.day().min(u32::from(days_in_month)));
    NaiveDate::from_ymd_opt(year, month, target_day).unwrap_or(on)
}

/// Whether an `Auto Repeat` row is due on `today`.
///
/// A row is due when it is enabled, its `end_date` (when set) has not passed,
/// and its `next_schedule_date` has arrived (falling back to `start_date` on
/// the first run, and to "now" when neither is set).
#[must_use]
pub fn is_due(row: &Value, today: NaiveDate) -> bool {
    if bool_field(row, "disabled", false) {
        return false;
    }
    if let Some(end) = date_field(row, "end_date")
        && today > end
    {
        return false;
    }
    if let Some(next) = date_field(row, "next_schedule_date") {
        return today >= next;
    }
    if let Some(start) = date_field(row, "start_date") {
        return today >= start;
    }
    true
}

/// Insert the fresh copy. Shared so a caller could preview the clone.
///
/// # Errors
///
/// Fails when the reference document cannot be read or the engine rejects the
/// insert (a validated field, duplicate name, or database error).
pub async fn clone_document(
    state: &IntegrationsContext,
    reference_doctype: &str,
    reference: &Value,
) -> Result<Value> {
    let mut cloned: HashMap<String, Value> = HashMap::new();
    if let Value::Object(map) = reference {
        for (key, value) in map {
            if SYSTEM_FIELDS.contains(&key.as_str()) {
                continue;
            }
            cloned.insert(key.clone(), value.clone());
        }
    }
    store::create(state, reference_doctype, Value::Object(cloned)).await
}

/// Create copies for every `Auto Repeat` row due today. Returns how many
/// documents were created.
///
/// # Errors
///
/// Fails when the `Auto Repeat` rows cannot be listed; per-row failures
/// (unreadable reference, rejected clone) are logged and skipped.
pub async fn run_due(state: &IntegrationsContext, today: NaiveDate) -> Result<usize> {
    let rows = store::list(state, AUTO_REPEAT, HashMap::new()).await?;
    let mut created = 0;
    for row in rows {
        if !is_due(&row, today) {
            continue;
        }
        let name = str_field(&row, "name");
        let reference_doctype = str_field(&row, "reference_doctype");
        let reference_document = str_field(&row, "reference_document");
        if name.is_empty() || reference_doctype.is_empty() || reference_document.is_empty() {
            continue;
        }
        match store::get(state, &reference_doctype, &reference_document).await {
            Ok(reference) => match clone_document(state, &reference_doctype, &reference).await {
                Ok(_) => created += 1,
                Err(e) => {
                    tracing::warn!(name, error = %e, "auto repeat failed to clone the reference document");
                }
            },
            Err(e) => {
                tracing::warn!(
                    name,
                    reference_doctype,
                    reference_document,
                    error = %e,
                    "auto repeat: reference document unavailable"
                );
            }
        }
        let frequency = str_field(&row, "frequency");
        let repeat_on_day = row.get("repeat_on_day").and_then(Value::as_i64);
        let base = date_field(&row, "next_schedule_date")
            .or_else(|| date_field(&row, "start_date"))
            .unwrap_or(today);
        // Roll forward from *at least today*: a row that just fired must not be
        // due again on the same day even when its stored dates lag behind (the
        // host was down, the row was imported, a manual edit rewound it).
        let next = advance(&frequency, base.max(today), repeat_on_day);
        if let Err(e) = store::update(
            state,
            AUTO_REPEAT,
            &name,
            Value::from_json(serde_json::json!({
                "next_schedule_date": next.to_string(),
            })),
        )
        .await
        {
            tracing::warn!(name, error = %e, "auto repeat failed to advance the schedule");
        }
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn advance_daily_weekly_and_monthly() {
        let base = dt(2026, 1, 15);
        assert_eq!(advance("Daily", base, None), dt(2026, 1, 16));
        assert_eq!(advance("weekly", base, None), dt(2026, 1, 22));
        assert_eq!(advance("Monthly", base, None), dt(2026, 2, 15));
        assert_eq!(advance("Quarterly", base, None), dt(2026, 4, 15));
        assert_eq!(advance("Yearly", base, None), dt(2027, 1, 15));
    }

    #[test]
    fn repeat_on_day_pins_the_day_and_clamps_to_month_end() {
        let base = dt(2026, 1, 31);
        assert_eq!(advance("Monthly", base, Some(1)), dt(2026, 2, 1));
        // January 31 -> repeat day 31 does not exist in February: clamp.
        assert_eq!(advance("Monthly", base, Some(31)), dt(2026, 2, 28));
        // No repeat day keeps the (clamped) day of month.
        assert_eq!(advance("Monthly", base, None), dt(2026, 2, 28));
    }

    #[test]
    fn unknown_frequency_advances_daily() {
        let base = dt(2026, 3, 1);
        assert_eq!(advance("Hourly", base, None), dt(2026, 3, 2));
    }

    #[test]
    fn is_due_obeys_dates_and_disabled() {
        let base = |key: &str, v: &str| (key.to_string(), Value::string(v));
        let next = Value::from_json(serde_json::json!({
            "name": "AR-1",
            "next_schedule_date": "2026-12-01",
        }));
        assert!(is_due(&next, dt(2026, 12, 1)));
        assert!(!is_due(&next, dt(2026, 11, 30)));

        let with_end = Value::from_json(serde_json::json!({
            "next_schedule_date": "2025-06-01",
            "end_date": "2025-12-31",
        }));
        assert!(!is_due(&with_end, dt(2026, 1, 2)), "past the end date");

        let disabled = Value::from_json(serde_json::json!({
            "disabled": true,
            "next_schedule_date": "2020-01-01",
        }));
        assert!(!is_due(&disabled, dt(2026, 1, 1)));

        let mut map = HashMap::from([base("name", "AR-2"), base("start_date", "2026-06-01")]);
        let _ = map.insert("frequency".into(), Value::string("Daily"));
        assert!(is_due(&Value::Object(map), dt(2026, 6, 2)));
    }
}
