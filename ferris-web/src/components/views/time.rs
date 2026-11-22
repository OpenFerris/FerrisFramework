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
/// Deliberately coarse: a timeline reads better with "hace 2 h" than with a
/// precise duration, and the exact stamp is always one hover away.
#[must_use]
pub fn relative_from(seconds: i64) -> String {
    let seconds = seconds.max(0);
    match seconds {
        0..=59 => "ahora".to_string(),
        60..=3599 => format!("hace {} min", seconds / 60),
        3600..=86_399 => format!("hace {} h", seconds / 3600),
        86_400..=172_799 => "ayer".to_string(),
        172_800..=2_591_999 => format!("hace {} días", seconds / 86_400),
        _ => String::new(),
    }
}

/// Relative age of a stamp ("hace 5 min"), or the exact date when it is old.
#[must_use]
pub fn relative(at: &str) -> String {
    let Some(stamp) = parse(at) else {
        return String::new();
    };
    let elapsed = (Utc::now() - stamp).num_seconds().max(0);
    let coarse = relative_from(elapsed);
    if coarse.is_empty() { exact(at) } else { coarse }
}

/// `dd/mm/yyyy hh:mm`, for tooltips.
#[must_use]
pub fn exact(at: &str) -> String {
    parse(at)
        .map(|stamp| stamp.format("%d/%m/%Y %H:%M").to_string())
        .unwrap_or_default()
}

/// Day bucket of a stamp: "Hoy", "Ayer", "12 nov" or "12 nov 2025".
///
/// The history groups its entries with this label, so a long timeline reads as
/// days instead of one undifferentiated list.
#[must_use]
pub fn day(at: &str) -> String {
    const MONTHS: [&str; 12] = [
        "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
    ];
    let Some(stamp) = parse(at) else {
        return String::new();
    };
    let today = Utc::now().date_naive();
    let date = stamp.date_naive();
    if date == today {
        return "Hoy".to_string();
    }
    if date.succ_opt() == Some(today) {
        return "Ayer".to_string();
    }
    let month = MONTHS[(date.month0() as usize).min(11)];
    if date.year() == today.year() {
        format!("{} {}", date.day(), month)
    } else {
        format!("{} {} {}", date.day(), month, date.year())
    }
}

#[cfg(test)]
mod tests {
    use super::{day, exact, relative_from};

    #[test]
    fn a_recent_stamp_reads_as_a_coarse_age() {
        assert_eq!(relative_from(0), "ahora");
        assert_eq!(relative_from(59), "ahora");
        assert_eq!(relative_from(60), "hace 1 min");
        assert_eq!(relative_from(3_600), "hace 1 h");
        assert_eq!(relative_from(86_400), "ayer");
        assert_eq!(relative_from(172_800), "hace 2 días");
        assert_eq!(relative_from(-5), "ahora");
    }

    #[test]
    fn the_exact_stamp_is_rendered_for_tooltips() {
        assert_eq!(exact("2026-11-18T10:05:00Z"), "18/11/2026 10:05");
        assert_eq!(exact("not a date"), "");
    }

    #[test]
    fn a_today_stamp_is_grouped_under_hoy() {
        let now = chrono::Utc::now();
        assert_eq!(day(&now.to_rfc3339()), "Hoy");
        let yesterday = now - chrono::Duration::days(1);
        assert_eq!(day(&yesterday.to_rfc3339()), "Ayer");
    }
}
