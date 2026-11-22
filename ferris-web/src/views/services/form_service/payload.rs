//! Payload builders for the form.
//!
//! Split from [`super::layout`] so the field model/validation and the wire
//! shaping are separate concerns, and because the child-table round-trip made
//! the payload rules big enough to matter. Everything here reads `FormField`
//! signals and returns the JSON body the engine stores; the same functions are
//! used by save, duplicate and the "dirty" fingerprint.

use leptos::prelude::*;
use serde_json::Value;

use crate::api::DynMeta;

use super::FormField;

/// Parse a `Table MultiSelect` value into the list of names it holds.
///
/// The **single** policy for that fieldtype's stored shape, shared by
/// validation, payload building and the `MultiLinkField` widget (which lives
/// in the components layer — this is the same `FormField` edge the renderer
/// already crosses): a JSON array of names, or — for a value that predates the
/// first edit — one bare name. Anything non-empty that is not an array
/// degrades to that single element instead of vanishing.
#[must_use]
pub fn parse_name_list(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    serde_json::from_str::<Vec<String>>(trimmed).unwrap_or_else(|_| vec![trimmed.to_string()])
}

/// Build the payload for create/update from form fields.
pub fn build_form_payload(fields: &[FormField]) -> Value {
    let mut map = serde_json::Map::new();
    for f in fields {
        if matches!(
            f.fieldtype.as_str(),
            "Tab Break"
                | "Section Break"
                | "Column Break"
                | "HTML"
                | "Button"
                | "Attachment Gallery"
                | "Heading"
                // A `Widget` field has no value; sending its empty string would
                // create (or clear) a column the widget never edits.
                | "Widget"
        ) {
            continue;
        }
        // The synthetic id field: an untouched naming-series template (`#`) or
        // an empty engine-owned id must not reach the payload, or the engine
        // would use the literal template as the primary key instead of minting
        // one. A value the user actually typed is sent as-is.
        if f.is_name {
            let raw = f.value.get_untracked();
            if raw.trim().is_empty() || raw.contains('#') {
                continue;
            }
        }
        // The payload builders run from event handlers, not inside a render:
        // the reads are snapshots (`get_untracked`), which is also what keeps
        // them from warning about being outside a reactive context.
        let v = f.value.get_untracked();
        match f.fieldtype.as_str() {
            "Check" => map.insert(f.fieldname.clone(), Value::Bool(f.checked.get_untracked())),
            // An `Int` is an integer on the wire. Sending the parsed `f64`
            // wrote `7.0`, which the engine rejects ("must be an integer"); the
            // whole save failed on a value the user typed correctly.
            "Int" => number_text(&v)
                .filter(|n| n.fract() == 0.0)
                .map(|n| n as i64)
                .and_then(|n| map.insert(f.fieldname.clone(), Value::from(n))),
            "Float" | "Currency" | "Percent" => {
                number_text(&v).and_then(|n| map.insert(f.fieldname.clone(), Value::from(n)))
            }
            // The one list policy (see `parse_name_list`): the wire value is
            // the JSON array the engine expects, never the widget's raw text.
            "Table MultiSelect" => {
                let names = parse_name_list(&v);
                if names.is_empty() {
                    None
                } else {
                    map.insert(
                        f.fieldname.clone(),
                        Value::Array(names.into_iter().map(Value::String).collect()),
                    )
                }
            }
            // Child rows: the editor stores the array as JSON in `value`; an
            // empty table is sent as `[]` so clearing every row reaches the
            // engine instead of leaving the previous rows in place. A value
            // that does not parse as an array is omitted, never turned into
            // `[]`: claiming "no rows" would delete the document's children
            // on a save the user did not even make in that field.
            "Table" => match serde_json::from_str::<Value>(&v) {
                Ok(Value::Array(rows)) => map.insert(f.fieldname.clone(), Value::Array(rows)),
                _ => None,
            },
            // A `JSON` field is stored as JSON, not as the text the editor
            // shows: sending the string made the engine reject the save with
            // "Value must be a JSON object or array". A scalar or invalid text
            // is omitted here; `validate_form` is what reports it to the user
            // before the request is made.
            "JSON" => match serde_json::from_str::<Value>(&v) {
                Ok(value @ (Value::Object(_) | Value::Array(_))) => {
                    map.insert(f.fieldname.clone(), value)
                }
                _ => None,
            },
            // The native control writes `YYYY-MM-DDTHH:MM[:SS]`; the engine
            // accepts RFC 3339 or `YYYY-MM-DD HH:MM:SS`. Without this
            // conversion every save of a form with a Datetime failed with
            // "Invalid Datetime format".
            "Datetime" => {
                let wire = datetime_wire(&v);
                if wire.is_empty() {
                    None
                } else {
                    map.insert(f.fieldname.clone(), Value::String(wire))
                }
            }
            _ => {
                if v.is_empty() {
                    None
                } else {
                    map.insert(f.fieldname.clone(), Value::String(v))
                }
            }
        };
    }
    Value::Object(map)
}

/// A numeric field's text as a number. The widget keeps the separator the user
/// typed (`.` or the locale's `,`); both are accepted here, so the wire value is
/// the same number either way.
fn number_text(raw: &str) -> Option<f64> {
    raw.trim().replace(',', ".").parse::<f64>().ok()
}

/// Normalize the native `datetime-local` value (`YYYY-MM-DDTHH:MM[:SS]`) to the
/// naive form the engine accepts (`YYYY-MM-DD HH:MM:SS`).
fn datetime_wire(raw: &str) -> String {
    let text = raw.trim();
    if text.is_empty() {
        return String::new();
    }
    let text = text.replace('T', " ");
    let (date, time) = text.split_once(' ').unwrap_or((text.as_str(), ""));
    if time.is_empty() {
        return date.to_string();
    }
    let time = time.trim_end_matches('Z');
    let time = time.split(['+', '-']).next().unwrap_or_default();
    if time.matches(':').count() >= 2 {
        format!("{date} {time}")
    } else {
        format!("{date} {time}:00")
    }
}

/// Drop virtual fields from a payload about to be written.
///
/// A Virtual DocField is computed on read and has no storage: sending the value
/// the form displays would make the engine reject the whole save with
/// `VirtualFieldReadOnly`. The metadata, not the rendered field, is the source
/// of truth, so the strip happens here even if the form still shows the value.
#[must_use]
pub fn strip_virtual_fields(payload: Value, meta: Option<&DynMeta>) -> Value {
    let Some(meta) = meta else {
        return payload;
    };
    let Value::Object(mut map) = payload else {
        return payload;
    };
    for field in meta.fields.iter().filter(|field| field.r#virtual) {
        map.remove(&field.fieldname);
    }
    Value::Object(map)
}

/// Body for a duplicate of the document currently in the form.
///
/// Starts from what the form shows, reusing [`build_form_payload`] — the same
/// builder the save uses, so a copy cannot contain anything the user could not
/// have written themselves — and then drops the two keys that would tie the copy
/// to its source: `name` identifies a document, and `docstatus` is that
/// document's place in the workflow. `owner`, `creation`, `modified`,
/// `modified_by`, `idx` and `_user_tags` need no explicit strip: none of them is
/// a form field, so `build_form_payload` never emits them and the engine fills
/// them in.
///
/// This is what C2 needed: `navigate_new()` alone copied nothing,
/// it just opened a blank form that then saved over the original.
pub fn duplicate_payload(fields: &[FormField]) -> Value {
    let mut payload = build_form_payload(fields);
    if let Value::Object(map) = &mut payload {
        map.remove("name");
        map.remove("docstatus");
    }
    payload
}
