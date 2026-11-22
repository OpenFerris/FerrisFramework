//! Filters available to print templates.
//!
//! They are deliberately small and total: no filter reaches the network, the
//! filesystem or a Rust type. A template gets formatting helpers over the JSON
//! context (`doc`, `params`, `meta`, `user`, `now`) and nothing else.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use minijinja::{Environment, Error, ErrorKind, Value};

use super::escape;

pub(super) fn register(env: &mut Environment<'static>) {
    env.add_filter("typst", |value: &Value| {
        escape::typst_markup(&value.to_string())
    });
    env.add_filter("typst_str", |value: &Value| {
        escape::typst_string(&value.to_string())
    });
    env.add_filter("typst_value", typst_value);
    env.add_filter("date", date_filter);
    env.add_filter("datetime", date_filter);
    env.add_filter("money", money_filter);
    env.add_filter("number", number_filter);
    env.add_filter("percent", percent_filter);
    env.add_filter("json", json_filter);
}

fn typst_value(value: &Value) -> Result<String, Error> {
    let json = serde_json::to_value(value)
        .map_err(|error| Error::new(ErrorKind::InvalidOperation, error.to_string()))?;
    Ok(escape::typst_value(&json))
}

/// Format a date/datetime. Strings are parsed as RFC 3339, `%Y-%m-%d
/// %H:%M:%S` or `%Y-%m-%d`; numbers are Unix seconds (or milliseconds when
/// they are large enough to be one).
fn date_filter(value: &Value, format: Option<String>) -> Result<String, Error> {
    let format = format.unwrap_or_else(|| "%Y-%m-%d".to_string());
    let date = parse_datetime(value)?;
    Ok(date.format(&format).to_string())
}

fn parse_datetime(value: &Value) -> Result<DateTime<Utc>, Error> {
    if let Some(raw) = value.as_str() {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(invalid("date value is empty"));
        }
        if let Ok(date) = DateTime::parse_from_rfc3339(raw) {
            return Ok(date.with_timezone(&Utc));
        }
        for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d"] {
            if let Ok(date) = NaiveDateTime::parse_from_str(raw, format) {
                return Ok(date.and_utc());
            }
            if let Ok(date) = NaiveDate::parse_from_str(raw, format) {
                let date = date
                    .and_hms_opt(0, 0, 0)
                    .ok_or_else(|| invalid("date is out of range"))?;
                return Ok(date.and_utc());
            }
        }
        if let Ok(seconds) = raw.parse::<i64>() {
            return from_timestamp(seconds);
        }
        return Err(invalid(format!("cannot parse date `{raw}`")));
    }
    if let Some(seconds) = value.as_i64() {
        return from_timestamp(seconds);
    }
    Err(invalid(
        "date filter expects a date string or a Unix timestamp",
    ))
}

fn from_timestamp(timestamp: i64) -> Result<DateTime<Utc>, Error> {
    // A value above this is almost certainly milliseconds. `unsigned_abs`
    // avoids the `i64::MIN` overflow `abs()` would panic on.
    let timestamp = if timestamp.unsigned_abs() > 10_000_000_000 {
        timestamp / 1000
    } else {
        timestamp
    };
    Utc.timestamp_opt(timestamp, 0)
        .single()
        .ok_or_else(|| invalid("timestamp is out of range"))
}

fn money_filter(
    value: &Value,
    decimals: Option<u32>,
    symbol: Option<String>,
) -> Result<String, Error> {
    let number = numeric(value)?;
    let rendered = format_decimal(number, decimals.unwrap_or(2) as usize);
    Ok(match symbol {
        Some(symbol) if !symbol.is_empty() => format!("{symbol}{rendered}"),
        _ => rendered,
    })
}

fn number_filter(value: &Value, decimals: Option<u32>) -> Result<String, Error> {
    let number = numeric(value)?;
    Ok(format_decimal(number, decimals.unwrap_or(2) as usize))
}

fn percent_filter(value: &Value, decimals: Option<u32>) -> Result<String, Error> {
    let number = numeric(value)? * 100.0;
    Ok(format!(
        "{}%",
        format_decimal(number, decimals.unwrap_or(1) as usize)
    ))
}

fn json_filter(value: &Value) -> Result<String, Error> {
    serde_json::to_string(value)
        .map_err(|error| Error::new(ErrorKind::InvalidOperation, error.to_string()))
}

// An i64 above 2^53 cannot be represented exactly as f64; formatting money and
// numbers is a display concern, so the rounding is intentional (and the value
// has already been accepted as numeric).
#[allow(clippy::cast_precision_loss)]
fn numeric(value: &Value) -> Result<f64, Error> {
    // MiniJinja 3 removed `Value::as_f64`; the fallible conversion is the
    // supported way to ask a value for a float.
    if let Ok(number) = f64::try_from(value.clone()) {
        return Ok(number);
    }
    if let Some(number) = value.as_i64() {
        return Ok(number as f64);
    }
    if let Some(raw) = value.as_str() {
        let raw = raw.trim().replace(',', "");
        if let Ok(number) = raw.parse::<f64>() {
            return Ok(number);
        }
    }
    Err(invalid(format!("expected a number, got `{value}`")))
}

/// Fixed-point formatting with thousands separators and no locale dependency
/// (a print must be deterministic across hosts).
fn format_decimal(value: f64, decimals: usize) -> String {
    let negative = value.is_sign_negative();
    let rendered = format!("{:.decimals$}", value.abs());
    let (integer, fraction) = match rendered.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction.to_string())),
        None => (rendered.as_str(), None),
    };
    let mut grouped = String::with_capacity(integer.len() + integer.len() / 3 + 1);
    for (index, ch) in integer.chars().enumerate() {
        if index > 0 && (integer.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    if negative {
        grouped.insert(0, '-');
    }
    match fraction {
        Some(fraction) => format!("{grouped}.{fraction}"),
        None => grouped,
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidOperation, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_parse_the_shapes_a_document_carries() {
        let value = Value::from("2026-11-21");
        assert_eq!(date_filter(&value, None).unwrap(), "2026-11-21");
        let value = Value::from("2026-11-21 13:45:00");
        assert_eq!(
            date_filter(&value, Some("%d/%m/%Y %H:%M".to_string())).unwrap(),
            "21/11/2026 13:45"
        );
    }

    #[test]
    fn money_and_numbers_are_grouped() {
        assert_eq!(
            money_filter(&Value::from(1_234_567.5), None, Some("€".to_string())).unwrap(),
            "€1,234,567.50"
        );
        assert_eq!(number_filter(&Value::from(42), Some(0)).unwrap(), "42");
    }

    #[test]
    fn an_invalid_number_is_an_error_not_a_zero() {
        let error = number_filter(&Value::from("not a number"), None).unwrap_err();
        assert!(error.to_string().contains("expected a number"), "{error}");
    }
}
