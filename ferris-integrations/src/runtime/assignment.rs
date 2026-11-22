//! Assignment rules: pick the matching rule on a document insert and notify the
//! assignee.
//!
//! Frappe assigns the document to the rule's user/role; Ferris has no mutable
//! assignment field on arbitrary documents, so the runtime notifies the
//! assignee and, when a `ToDo` DocType is registered (the Build workspace
//! declares it), creates the tracking row — the same observable contract, built
//! from what the framework owns.
//!
//! Matching picks the enabled, condition-passing rule with the highest
//! `priority` (ties keep the first row the engine returns).

use std::collections::HashMap;

use ferris_core::{Result, Value};

use super::condition;
use super::names::{ASSIGNMENT_RULE, TODO};
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

/// Apply the best `Assignment Rule` for a newly-inserted `doctype`/`docname`.
///
/// Returns the number of assignments made (0 or 1). Errors constructing the
/// notification are returned so a caller (a hook) can decide how loud to be.
///
/// # Errors
///
/// Fails only when the assignment-rule lookup or the notification insert fails;
/// a condition that does not parse is logged and skipped, never fatal.
pub async fn apply_for_insert(
    state: &IntegrationsContext,
    doctype: &str,
    docname: &str,
    fields: &ValueMap,
) -> Result<usize> {
    let rows = store::list(
        state,
        ASSIGNMENT_RULE,
        HashMap::from([("document_type".to_string(), Value::string(doctype))]),
    )
    .await?;

    let mut best: Option<(i64, Value)> = None;
    for row in rows {
        if bool_field(&row, "disabled", false) {
            continue;
        }
        let condition_text = str_field(&row, "condition");
        match condition::evaluate_optional(
            (!condition_text.is_empty()).then_some(condition_text.as_str()),
            fields,
        ) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                tracing::warn!(doctype, error = %e, "assignment rule condition failed; skipping");
                continue;
            }
        }
        let priority = row.get("priority").and_then(Value::as_i64).unwrap_or(0);
        let is_better = best.as_ref().is_none_or(|(current, _)| priority > *current);
        if is_better {
            best = Some((priority, row));
        }
    }

    let Some((_, rule)) = best else {
        return Ok(0);
    };
    let user = str_field(&rule, "assign_to");
    if user.is_empty() {
        return Ok(0);
    }

    let title = format!("Assigned: {doctype} {docname}");
    let message = format!("{doctype} {docname} was assigned to you.");
    let route = format!("/app/{doctype}/{docname}");
    state.notify(&user, &title, &message, &route).await?;

    // Frappe mirrors an assignment in To Do; when the DocType exists we do too.
    if store::is_registered(TODO) {
        let data = Value::from_json(serde_json::json!({
            "type": doctype,
            "description": format!("{docname} assigned by rule"),
            "status": "Open",
        }));
        if let Err(e) = store::create_owned_by(state, TODO, &user, data).await {
            tracing::warn!(error = %e, "assignment rule failed to create a ToDo row");
        }
    }
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_rule_selection_is_deterministic() {
        let make = |priority: i64, disabled: bool| {
            Value::from_json(serde_json::json!({
                "name": format!("R-{priority}"),
                "priority": priority,
                "disabled": disabled,
                "assign_to": format!("user-{priority}"),
                "condition": "",
            }))
        };
        let rows = vec![make(10, false), make(50, false), make(100, true)];
        let mut best: Option<(i64, Value)> = None;
        for row in rows {
            if bool_field(&row, "disabled", false) {
                continue;
            }
            let priority = row.get("priority").and_then(Value::as_i64).unwrap_or(0);
            let is_better = best.as_ref().is_none_or(|(current, _)| priority > *current);
            if is_better {
                best = Some((priority, row));
            }
        }
        let (priority, rule) = best.unwrap();
        assert_eq!(priority, 50);
        assert_eq!(str_field(&rule, "assign_to"), "user-50");
    }
}
