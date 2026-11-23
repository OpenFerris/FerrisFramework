//! Human times for the Desk.
//!
//! The engine stores RFC 3339 stamps. No screen prints one raw: the list shows
//! an age, provenance shows "hace 5 min" with the exact date on hover, and the
//! history groups entries under "Hoy"/"Ayer"/a date.
//!
//! Everything is a pure function of `now` (split out so the tests do not depend
//! on the wall clock): the component reads the clock once and passes the elapsed
//! seconds.

use chrono::{DateTime, Datelike, Utc};

/// Parse an RFC 3339 stamp into UTC.
fn parse(at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|stamp| stamp.with_timezone(&Utc))
}

/// Relative age for a number of elapsed seconds.
///
/// Deliberately coarse: a timeline reads better with "2 h ago" than with a
/// precise duration, and the exact stamp is always one hover away.
#[must_use]
pub fn relative_from(seconds: i64, i18n: &(impl crate::i18n::Translate + Sync)) -> String {
    let seconds = seconds.max(0);
    let count = |value: i64| value.to_string();
    match seconds {
        0..=59 => i18n.t("components.time.now"),
        60..=3599 => i18n.t_with(
            "components.time.minutes_ago",
            &[("count", &count(seconds / 60))],
        ),
        3600..=86_399 => i18n.t_with(
            "components.time.hours_ago",
            &[("count", &count(seconds / 3600))],
        ),
        86_400..=172_799 => i18n.t("components.time.yesterday"),
        172_800..=2_591_999 => i18n.t_with(
            "components.time.days_ago",
            &[("count", &count(seconds / 86_400))],
        ),
        _ => String::new(),
    }
}

/// Relative age of a stamp ("5 min ago"), or the exact date when it is old.
#[must_use]
pub fn relative(at: &str, i18n: &(impl crate::i18n::Translate + Sync)) -> String {
    let Some(stamp) = parse(at) else {
        return String::new();
    };
    let elapsed = (Utc::now() - stamp).num_seconds().max(0);
    let coarse = relative_from(elapsed, i18n);
    if coarse.is_empty() { exact(at) } else { coarse }
}

/// `dd/mm/yyyy hh:mm`, for tooltips.
#[must_use]
pub fn exact(at: &str) -> String {
    parse(at)
        .map(|stamp| stamp.format("%d/%m/%Y %H:%M").to_string())
        .unwrap_or_default()
}

/// Day bucket of a stamp: "Today", "Yesterday", "12 Nov" or "12 Nov 2025".
///
/// The history groups its entries with this label, so a long timeline reads as
/// days instead of one undifferentiated list.
#[must_use]
pub fn day(at: &str, i18n: &(impl crate::i18n::Translate + Sync)) -> String {
    let Some(stamp) = parse(at) else {
        return String::new();
    };
    let today = Utc::now().date_naive();
    let date = stamp.date_naive();
    if date == today {
        return i18n.t("components.time.today");
    }
    if date.succ_opt() == Some(today) {
        return i18n.t("components.time.yesterday");
    }
    let month_key = format!("components.time.month.{}", date.month());
    let month = i18n.t(&month_key);
    if date.year() == today.year() {
        format!("{} {}", date.day(), month)
    } else {
        format!("{} {} {}", date.day(), month, date.year())
    }
}

#[cfg(test)]
mod tests {
    use super::{day, exact, relative_from};
    use crate::i18n::DeskI18n;

    fn es() -> DeskI18n {
        DeskI18n::from_catalog(
            "es",
            &serde_json::json!({
                "components.time.now": "ahora",
                "components.time.minutes_ago": "hace {count} min",
                "components.time.hours_ago": "hace {count} h",
                "components.time.yesterday": "ayer",
                "components.time.days_ago": "hace {count} días",
                "components.time.today": "Hoy",
                "components.time.month.1": "ene",
                "components.time.month.2": "feb",
                "components.time.month.3": "mar",
                "components.time.month.4": "abr",
                "components.time.month.5": "may",
                "components.time.month.6": "jun",
                "components.time.month.7": "jul",
                "components.time.month.8": "ago",
                "components.time.month.9": "sep",
                "components.time.month.10": "oct",
                "components.time.month.11": "nov",
                "components.time.month.12": "dic",
            }),
        )
    }

    #[test]
    fn a_recent_stamp_reads_as_a_coarse_age() {
        let i18n = es();
        assert_eq!(relative_from(0, &i18n), "ahora");
        assert_eq!(relative_from(59, &i18n), "ahora");
        assert_eq!(relative_from(60, &i18n), "hace 1 min");
        assert_eq!(relative_from(3_600, &i18n), "hace 1 h");
        assert_eq!(relative_from(86_400, &i18n), "ayer");
        assert_eq!(relative_from(172_800, &i18n), "hace 2 días");
        assert_eq!(relative_from(-5, &i18n), "ahora");
    }

    #[test]
    fn the_exact_stamp_is_rendered_for_tooltips() {
        assert_eq!(exact("2026-11-18T10:05:00Z"), "18/11/2026 10:05");
        assert_eq!(exact("not a date"), "");
    }

    #[test]
    fn a_today_stamp_is_grouped_under_hoy() {
        let i18n = es();
        let now = chrono::Utc::now();
        assert_eq!(day(&now.to_rfc3339(), &i18n), "Hoy");
        let yesterday = now - chrono::Duration::days(1);
        assert_eq!(day(&yesterday.to_rfc3339(), &i18n), "ayer");
    }
}
