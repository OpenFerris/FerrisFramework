use super::*;
use serde_json::json;

fn es() -> crate::i18n::DeskI18n {
    crate::i18n::DeskI18n::from_catalog(
        "es",
        &json!({
            "components.time.now": "ahora",
            "components.time.unit.minute": "m",
            "components.time.unit.hour": "h",
            "components.time.unit.day": "d",
            "components.time.unit.month": "me",
        }),
    )
}

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const MONTH: i64 = 30 * DAY;

#[test]
fn age_picks_the_largest_fitting_unit() {
    assert_eq!(format_age(0, &es()), "ahora");
    assert_eq!(format_age(59, &es()), "ahora");
    assert_eq!(format_age(MINUTE, &es()), "1m");
    assert_eq!(format_age(2 * MINUTE, &es()), "2m");
    assert_eq!(format_age(HOUR - 1, &es()), "59m");
    assert_eq!(format_age(2 * HOUR, &es()), "2h");
    assert_eq!(format_age(DAY - 1, &es()), "23h");
    assert_eq!(format_age(3 * DAY, &es()), "3d");
    assert_eq!(format_age(MONTH - 1, &es()), "29d");
    assert_eq!(format_age(MONTH, &es()), "1me");
    assert_eq!(format_age(14 * MONTH, &es()), "14me");
}

#[test]
fn row_without_a_usable_stamp_still_lines_up() {
    assert_eq!(
        stamp_age("modified", &json!({}), &es()),
        ("—".into(), String::new())
    );
    assert_eq!(
        stamp_age("modified", &json!({ "modified": null }), &es()),
        ("—".into(), String::new())
    );
    assert_eq!(
        stamp_age("modified", &json!({ "modified": "" }), &es()),
        ("—".into(), String::new())
    );
    // A stamp belonging to another field does not stand in for this one.
    assert_eq!(
        stamp_age("creation", &json!({ "modified": "" }), &es()),
        ("—".into(), String::new())
    );
}

#[test]
fn a_stamp_in_the_future_reads_as_now() {
    let future = chrono::Utc::now() + chrono::Duration::hours(2);
    let (age, exact) = stamp_age(
        "modified",
        &json!({ "modified": future.to_rfc3339() }),
        &es(),
    );

    assert_eq!(age, "ahora");
    assert!(!exact.is_empty());
}

/// `creation` is listed as a column, so it ages the same way `modified`
/// does instead of showing the raw stamp.
#[test]
fn creation_ages_like_modified() {
    let three_days_ago = chrono::Utc::now() - chrono::Duration::days(3);
    let (age, exact) = stamp_age(
        "creation",
        &json!({ "creation": three_days_ago.to_rfc3339() }),
        &es(),
    );

    assert_eq!(age, "3d");
    assert!(!exact.is_empty());
}

#[test]
fn loading_more_waits_until_the_window_nears_the_end() {
    // Far from the end: no request yet.
    assert!(!should_load_more(5, 20, true, false));
    // Within the lookahead: request the next page.
    assert!(should_load_more(16, 20, true, false));
    // Already fetching: do not stack requests.
    assert!(!should_load_more(20, 20, true, true));
    // Exhausted: nothing left to request.
    assert!(!should_load_more(20, 20, false, false));
    // A reset with no rows must not ask for page two of nothing.
    assert!(!should_load_more(0, 0, true, false));
}
