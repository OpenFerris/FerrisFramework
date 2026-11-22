use super::{
    WEEKDAYS, event_range, grid_start, month_cells, month_start, parse_day, week_cells, week_start,
};
use chrono::{Datelike, NaiveDate};
use serde_json::json;

fn day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("valid date")
}

#[test]
fn a_month_grid_is_six_full_weeks_starting_on_monday() {
    let cells = month_cells(day(2026, 11, 15));
    assert_eq!(cells.len(), 42);
    assert_eq!(cells[0], day(2026, 10, 26), "Monday of the week of the 1st");
    assert_eq!(cells[0].weekday(), chrono::Weekday::Mon);
    assert_eq!(cells[41], day(2026, 12, 6));
    assert!(cells.contains(&day(2026, 11, 1)));
    assert!(cells.contains(&day(2026, 11, 30)));
}

#[test]
fn a_month_that_starts_on_monday_still_starts_the_grid_there() {
    let cells = month_cells(day(2026, 6, 10));
    assert_eq!(cells[0], day(2026, 6, 1));
}

#[test]
fn a_week_starts_on_monday_and_has_seven_days() {
    assert_eq!(week_start(day(2026, 11, 21)), day(2026, 11, 16));
    let cells = week_cells(day(2026, 11, 21));
    assert_eq!(cells.len(), 7);
    assert_eq!(cells[6], day(2026, 11, 22));
    assert_eq!(WEEKDAYS[0], "L");
}

#[test]
fn month_start_and_grid_start_are_the_documented_days() {
    assert_eq!(month_start(day(2026, 11, 30)), day(2026, 11, 1));
    assert_eq!(grid_start(day(2026, 11, 30)), day(2026, 10, 26));
}

#[test]
fn a_document_appears_on_each_day_it_spans() {
    let row = json!({ "name": "A", "start": "2026-11-20", "end": "2026-11-22" });
    let range = event_range(
        &row,
        "start",
        Some("end"),
        day(2026, 11, 1),
        day(2026, 11, 30),
    )
    .expect("inside the month");
    assert_eq!(range, (day(2026, 11, 20), day(2026, 11, 22)));
}

#[test]
fn a_range_is_clipped_to_the_visible_window() {
    let row = json!({ "name": "A", "start": "2026-10-28", "end": "2026-11-03" });
    let range = event_range(
        &row,
        "start",
        Some("end"),
        day(2026, 11, 1),
        day(2026, 11, 30),
    )
    .expect("overlaps");
    assert_eq!(range, (day(2026, 11, 1), day(2026, 11, 3)));
}

#[test]
fn a_document_outside_the_window_is_not_placed() {
    let row = json!({ "name": "A", "start": "2026-09-01", "end": "2026-09-02" });
    assert!(
        event_range(
            &row,
            "start",
            Some("end"),
            day(2026, 11, 1),
            day(2026, 11, 30)
        )
        .is_none()
    );
}

#[test]
fn an_end_before_the_start_is_the_start_day() {
    let row = json!({ "name": "A", "start": "2026-11-20", "end": "2026-11-01" });
    let range = event_range(
        &row,
        "start",
        Some("end"),
        day(2026, 11, 1),
        day(2026, 11, 30),
    )
    .expect("placed");
    assert_eq!(range, (day(2026, 11, 20), day(2026, 11, 20)));
}

#[test]
fn a_document_without_a_date_is_not_invented_into_the_grid() {
    let row = json!({ "name": "A" });
    assert!(event_range(&row, "creation", None, day(2026, 11, 1), day(2026, 11, 30)).is_none());
}

#[test]
fn a_timestamp_names_the_same_day() {
    assert_eq!(
        parse_day(&json!("2026-11-21T10:00:00")),
        Some(day(2026, 11, 21))
    );
    assert_eq!(parse_day(&json!("hello")), None);
}
