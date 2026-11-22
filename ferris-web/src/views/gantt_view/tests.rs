use super::{bars_and_timeline, covered_days, parse_day, progress_of};
use serde_json::json;

fn row(name: &str, start: &str, end: &str) -> serde_json::Value {
    json!({
        "name": name,
        "title": format!("Task {name}"),
        "start_date": start,
        "end_date": end,
        "percent": 40,
        "depends_on": "",
    })
}

#[test]
fn a_date_or_a_timestamp_both_name_a_day() {
    assert_eq!(
        parse_day(&json!("2026-11-21")),
        Some(chrono::NaiveDate::from_ymd_opt(2026, 11, 21).unwrap())
    );
    assert_eq!(
        parse_day(&json!("2026-11-21T10:00:00+00:00")),
        Some(chrono::NaiveDate::from_ymd_opt(2026, 11, 21).unwrap())
    );
    assert_eq!(parse_day(&json!("hello")), None);
    assert_eq!(parse_day(&json!(null)), None);
}

#[test]
fn bars_are_relative_to_the_earliest_start() {
    let rows = vec![
        row("A", "2026-11-01", "2026-11-03"),
        row("B", "2026-11-05", "2026-11-06"),
    ];
    let (bars, timeline) = bars_and_timeline(
        &rows,
        "start_date",
        "end_date",
        "title",
        Some("percent"),
        None,
    );
    let timeline = timeline.expect("a timeline");
    assert_eq!(
        timeline.origin,
        chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()
    );
    assert_eq!(timeline.days, 6);
    assert!(!timeline.truncated);
    assert_eq!(bars[0].start, 0);
    assert_eq!(bars[0].end, 2);
    assert_eq!(bars[1].start, 4);
    assert_eq!(bars[0].progress, Some(40));
}

#[test]
fn a_missing_start_is_not_invented() {
    let rows = vec![
        json!({ "name": "A", "end_date": "2026-11-03" }),
        row("B", "2026-11-01", "2026-11-02"),
    ];
    let (bars, timeline) = bars_and_timeline(&rows, "start_date", "end_date", "title", None, None);
    assert_eq!(bars.len(), 1);
    assert_eq!(bars[0].name, "B");
    assert!(timeline.is_some());
}

#[test]
fn an_end_before_the_start_is_one_day() {
    let rows = vec![row("A", "2026-11-10", "2026-11-01")];
    let (bars, timeline) = bars_and_timeline(&rows, "start_date", "end_date", "title", None, None);
    let timeline = timeline.expect("a timeline");
    assert_eq!(timeline.days, 1);
    assert_eq!(bars[0].start, bars[0].end);
}

#[test]
fn the_window_is_cut_at_the_declared_limit_and_says_so() {
    let rows = vec![row("A", "2026-01-01", "2026-06-30")];
    let (_, timeline) = bars_and_timeline(&rows, "start_date", "end_date", "title", None, None);
    let timeline = timeline.expect("a timeline");
    assert_eq!(timeline.days, super::MAX_DAYS);
    assert!(timeline.truncated);
}

#[test]
fn covered_days_are_clipped_to_the_window() {
    let rows = vec![
        row("A", "2026-01-01", "2026-06-30"),
        row("B", "2026-01-02", "2026-01-02"),
    ];
    let (bars, timeline) = bars_and_timeline(&rows, "start_date", "end_date", "title", None, None);
    let timeline = timeline.expect("a timeline");
    let covered = covered_days(&bars[0], timeline.days);
    assert_eq!(
        i64::try_from(covered.len()).unwrap_or(i64::MAX),
        timeline.days
    );
    assert!(covered.contains(&0));
    assert!(!covered.contains(&timeline.days));
}

#[test]
fn progress_is_clamped_and_missing_values_stay_missing() {
    assert_eq!(progress_of(&json!({ "p": 140 }), Some("p")), Some(100));
    assert_eq!(progress_of(&json!({ "p": -3 }), Some("p")), Some(0));
    assert_eq!(progress_of(&json!({ "p": "55" }), Some("p")), Some(55));
    assert_eq!(progress_of(&json!({}), Some("p")), None);
    assert_eq!(progress_of(&json!({ "p": 10 }), None), None);
}

#[test]
fn a_dependency_is_resolved_to_the_predecessors_title() {
    let mut first = row("A", "2026-11-01", "2026-11-03");
    first["title"] = json!("Diseño");
    let mut second = row("B", "2026-11-04", "2026-11-05");
    second["depends_on"] = json!("A");
    let (bars, _) = bars_and_timeline(
        &[first, second],
        "start_date",
        "end_date",
        "title",
        None,
        Some("depends_on"),
    );
    assert_eq!(bars[1].after.as_deref(), Some("Diseño"));
}
