//! Google API endpoints and the `Value` field accessors every call shares.

use ferris_core::Value;

/// `https://oauth2.googleapis.com/token`
pub const GOOGLE_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
/// `https://people.googleapis.com/v1/people/me/connections`
pub const GOOGLE_PEOPLE_CONNECTIONS_URI: &str =
    "https://people.googleapis.com/v1/people/me/connections";
/// `https://www.googleapis.com/calendar/v3/users/me/calendarList`
pub const GOOGLE_CALENDAR_LIST_URI: &str =
    "https://www.googleapis.com/calendar/v3/users/me/calendarList";

pub(super) fn str_field(doc: &Value, key: &str) -> String {
    doc.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

pub(super) fn bool_field(doc: &Value, key: &str, default: bool) -> bool {
    doc.get(key).and_then(Value::as_bool).unwrap_or(default)
}
