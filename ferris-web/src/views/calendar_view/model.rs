//! Calendar arithmetic and the date rules a row follows.
//!
//! Pure date math, kept out of the component so the grid, the paging and the
//! multi-day spans are testable without a browser.

use chrono::{Datelike, Duration, NaiveDate};
use serde_json::Value;

/// Month or week: the two scales the grid offers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Month,
    Week,
}

/// The first day of `date`'s month.
#[must_use]
pub fn month_start(date: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(date.year(), date.month(), 1).unwrap_or(date)
}

/// The Monday of the week containing `date`.
#[must_use]
pub fn week_start(date: NaiveDate) -> NaiveDate {
    let offset = i64::from(date.weekday().num_days_from_monday());
    date - Duration::days(offset)
}

/// The first cell of the month grid: the Monday of the week the 1st falls in.
#[must_use]
pub fn grid_start(month: NaiveDate) -> NaiveDate {
    week_start(month_start(month))
}

/// The 42 days (six weeks) the month grid shows, in reading order.
#[must_use]
pub fn month_cells(anchor: NaiveDate) -> Vec<NaiveDate> {
    let start = grid_start(anchor);
    (0..42).map(|day| start + Duration::days(day)).collect()
}

/// The seven days of the week `anchor` falls in.
#[must_use]
pub fn week_cells(anchor: NaiveDate) -> Vec<NaiveDate> {
    let start = week_start(anchor);
    (0..7).map(|day| start + Duration::days(day)).collect()
}

/// Parse the date part of a document value.
#[must_use]
pub fn parse_day(value: &Value) -> Option<NaiveDate> {
    let text = value.as_str()?;
    let head: String = text.chars().take(10).collect();
    NaiveDate::parse_from_str(&head, "%Y-%m-%d").ok()
}

/// The days a document covers, clipped to `[from, to]`.
///
/// A row without a usable start is skipped (it cannot be placed). An end before
/// the start is read as the start day, never as a negative span.
#[must_use]
pub fn event_range(
    row: &Value,
    start_field: &str,
    end_field: Option<&str>,
    from: NaiveDate,
    to: NaiveDate,
) -> Option<(NaiveDate, NaiveDate)> {
    let start = row.get(start_field).and_then(parse_day)?;
    let end = end_field
        .and_then(|field| row.get(field))
        .and_then(parse_day)
        .filter(|end| *end >= start)
        .unwrap_or(start);
    if end < from || start > to {
        return None;
    }
    Some((start.max(from), end.min(to)))
}

/// The Monday-to-Sunday header labels.
pub const WEEKDAYS: [&str; 7] = ["L", "M", "X", "J", "V", "S", "D"];

const fn month_long_key(month: u32) -> &'static str {
    match month {
        1 => "views.common.month_long.1",
        2 => "views.common.month_long.2",
        3 => "views.common.month_long.3",
        4 => "views.common.month_long.4",
        5 => "views.common.month_long.5",
        6 => "views.common.month_long.6",
        7 => "views.common.month_long.7",
        8 => "views.common.month_long.8",
        9 => "views.common.month_long.9",
        10 => "views.common.month_long.10",
        11 => "views.common.month_long.11",
        12 => "views.common.month_long.12",
        _ => "",
    }
}

/// The key of the abbreviated month name (`1` = January).
const fn month_short_key(month: u32) -> &'static str {
    match month {
        1 => "views.common.month_short.1",
        2 => "views.common.month_short.2",
        3 => "views.common.month_short.3",
        4 => "views.common.month_short.4",
        5 => "views.common.month_short.5",
        6 => "views.common.month_short.6",
        7 => "views.common.month_short.7",
        8 => "views.common.month_short.8",
        9 => "views.common.month_short.9",
        10 => "views.common.month_short.10",
        11 => "views.common.month_short.11",
        12 => "views.common.month_short.12",
        _ => "",
    }
}

pub fn month_label(date: NaiveDate, i18n: &(impl crate::i18n::Translate + Sync)) -> String {
    i18n.t_with(
        "views.calendar.month_label",
        &[
            ("month", &i18n.t(month_long_key(date.month()))),
            ("year", &date.year().to_string()),
        ],
    )
}

/// `2026-11-21` as `21 nov`.
pub fn short_day(date: NaiveDate, i18n: &(impl crate::i18n::Translate + Sync)) -> String {
    i18n.t_with(
        "views.calendar.short_day",
        &[
            ("day", &date.day().to_string()),
            ("month", &i18n.t(month_short_key(date.month()))),
        ],
    )
}

/// The time a row carries, when its date field is a `Datetime`.
pub fn when_label(row: &Value, field: &str) -> String {
    row.get(field)
        .and_then(Value::as_str)
        .filter(|text| text.len() > 10)
        .and_then(|text| text.get(11..16))
        .unwrap_or_default()
        .to_string()
}
