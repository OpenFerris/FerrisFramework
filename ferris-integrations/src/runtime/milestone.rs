//! Milestones: watch one field of a document and mark the milestone achieved
//! when it reaches the configured value.
//!
//! The `Milestone` row names `document_type`/`document_name`, the
//! `track_field` to watch, and the `value` that achieves it. The runtime runs
//! on every `OnUpdate` (which the engine also fires for a fresh insert, so a
//! document created already at its target is captured too) and compares the
//! field's current value for equality — the same observable rule Frappe's
//! tracking implements, without a diff against history that a stateless hook
//! cannot see.
//!
//! Comparison is string-normalized (numbers and booleans render like the Desk
//! editors store them), so `value = 100` matches a numeric field `100`.

use std::collections::HashMap;

use chrono::Utc;
use ferris_core::{Result, Value};

use super::names::MILESTONE;
use super::state::{IntegrationsContext, ValueMap};
use super::store;

fn str_field(doc: &Value, key: &str) -> String {
    doc.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn bool_field(doc: &Value, key: &str, default: bool) -> bool {
    doc.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// Normalize a field value the same way the condition evaluator does, so
/// `100`, `"100"` and `true` compare against a `value` text field sensibly.
fn as_text(value: &Value) -> String {
    if let Some(s) = value.as_str() {
        return s.trim().to_string();
    }
    if let Some(b) = value.as_bool() {
        return b.to_string();
    }
    if let Some(i) = value.as_i64() {
        return i.to_string();
    }
    if let Some(f) = value.as_f64() {
        return super::condition::fmt_f64(f);
    }
    if value.is_null() {
        return String::new();
    }
    serde_json::to_string(&value.to_json()).unwrap_or_default()
}

/// Flag every not-yet-achieved `Milestone` for `doctype`/`docname` whose
/// tracked field now equals its target value. Returns how many were achieved.
///
/// # Errors
///
/// Fails when the `Milestone` rows cannot be listed or an achieved row cannot
/// be persisted.
pub async fn check(
    state: &IntegrationsContext,
    doctype: &str,
    docname: &str,
    fields: &ValueMap,
) -> Result<usize> {
    let rows = store::list(
        state,
        MILESTONE,
        HashMap::from([
            ("document_type".to_string(), Value::string(doctype)),
            ("document_name".to_string(), Value::string(docname)),
        ]),
    )
    .await?;

    let now = Utc::now().to_rfc3339();
    let mut achieved = 0;
    for row in rows {
        if bool_field(&row, "achieved", false) {
            continue;
        }
        let track_field = str_field(&row, "track_field");
        let target = str_field(&row, "value");
        if track_field.is_empty() || target.is_empty() {
            continue;
        }
        let matches = fields
            .get(&track_field)
            .map(as_text)
            .is_some_and(|current| current == target);
        if !matches {
            continue;
        }
        let name = str_field(&row, "name");
        if name.is_empty() {
            continue;
        }
        store::update(
            state,
            MILESTONE,
            &name,
            Value::from_json(serde_json::json!({
                "achieved": true,
                "achieved_on": now,
            })),
        )
        .await?;
        achieved += 1;
    }
    Ok(achieved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_text_normalizes_numbers_bools_and_strings() {
        assert_eq!(as_text(&Value::from(100)), "100");
        assert_eq!(as_text(&Value::from("100")), "100");
        assert_eq!(as_text(&Value::from(true)), "true");
        assert_eq!(as_text(&Value::from(10.0)), "10");
    }
}
