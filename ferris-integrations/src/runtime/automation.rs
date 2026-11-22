//! Automation flows: evaluate conditions on document events and run actions.
//!
//! A document event matches an enabled `Automation Flow` whose `document_type`
//! and `event` line up and whose `condition` (blank = always) evaluates true.
//! Every match is enqueued as an `Automation Trigger Queue` row (Queued →
//! Running → Completed/Failed) and then processed immediately; the same
//! processor is exposed as a job handler ([`super::jobs`]) so a host can defer
//! processing instead.
//!
//! Actions are a JSON array of objects — `{ "type": "notification", "user",
//! "title", "message" }` and `{ "type": "assign", "user" }` — with `{{ doc.* }}`
//! template interpolation against the triggering document.

use std::collections::HashMap;

use chrono::Utc;
use ferris_core::{Result, Value};

use super::condition;
use super::names::{AUTOMATION_FLOW, AUTOMATION_TRIGGER_QUEUE, TODO};
use super::state::{IntegrationsContext, ValueMap};
use super::{store, template};

pub use super::condition::evaluate as evaluate_condition;

/// What one action object asks the runtime to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// Raise an in-app [`Notification`](super::names::NOTIFICATION) for a user.
    Notification,
    /// Assign the document to a user (notification + `ToDo` row when declared).
    Assign,
}

/// One parsed `actions` entry.
#[derive(Debug, Clone)]
pub struct Action {
    pub kind: ActionKind,
    pub title: Option<String>,
    pub message: Option<String>,
    pub user: Option<String>,
}

fn str_field(doc: &Value, key: &str) -> String {
    doc.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn bool_field(doc: &Value, key: &str, default: bool) -> bool {
    doc.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// Fields snapshot of a stored document (empty when the row is gone).
fn fields_of(doc: &Value) -> HashMap<String, Value> {
    match doc {
        Value::Object(map) => map.clone(),
        _ => HashMap::new(),
    }
}

/// Parse the `actions` text field. Unknown action types are logged and skipped
/// so one bad entry does not disable the whole flow.
pub fn parse_actions(text: &str) -> Vec<Action> {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let items: Vec<&serde_json::Value> = match &parsed {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(_) => vec![&parsed],
        _ => return Vec::new(),
    };
    items.into_iter().filter_map(parse_action).collect()
}

/// The `actions` cell as parseable text. The DocType declares a `Code` (text)
/// field, but a loose JSON author may write the array form directly; accept
/// both rather than silently running nothing.
fn actions_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(&other.to_json()).unwrap_or_default(),
    }
}

fn parse_action(item: &serde_json::Value) -> Option<Action> {
    let obj = item.as_object()?;
    let kind = obj.get("type").or_else(|| obj.get("action"))?.as_str()?;
    let kind = match kind {
        "notification" | "notify" => ActionKind::Notification,
        "assign" => ActionKind::Assign,
        other => {
            tracing::warn!(kind = %other, "automation action type ignored (use notification|assign)");
            return None;
        }
    };
    let string_of = |key: &str| obj.get(key).and_then(|v| v.as_str()).map(str::to_string);
    Some(Action {
        kind,
        title: string_of("title"),
        message: string_of("message").or_else(|| string_of("description")),
        user: string_of("user").or_else(|| string_of("assign_to")),
    })
}

/// Run the actions of one matched flow against the triggering document.
async fn run_actions(
    state: &IntegrationsContext,
    actions: &[Action],
    doctype: &str,
    docname: &str,
    fields: &HashMap<String, Value>,
) -> Result<()> {
    let route = format!("/app/{doctype}/{docname}");
    for action in actions {
        match action.kind {
            ActionKind::Notification => {
                let Some(recipient) = action.user.clone() else {
                    tracing::warn!("automation notification action has no `user`; skipped");
                    continue;
                };
                let title = template::render(
                    action.title.as_deref().unwrap_or("Automation"),
                    doctype,
                    docname,
                    fields,
                );
                let message = template::render(
                    action.message.as_deref().unwrap_or(""),
                    doctype,
                    docname,
                    fields,
                );
                state.notify(&recipient, &title, &message, &route).await?;
            }
            ActionKind::Assign => {
                let Some(user) = action.user.clone() else {
                    tracing::warn!("automation assign action has no `user`; skipped");
                    continue;
                };
                let title = format!("Assigned: {doctype} {docname}");
                let message = format!("{doctype} {docname} was assigned to you.");
                state.notify(&user, &title, &message, &route).await?;
                if store::is_registered(TODO) {
                    let data = Value::from_json(serde_json::json!({
                        "type": doctype,
                        "description": format!("{docname} assigned by automation"),
                        "status": "Open",
                    }));
                    if let Err(e) = store::create_owned_by(state, TODO, &user, data).await {
                        tracing::warn!(error = %e, "automation failed to create a ToDo row");
                    }
                }
            }
        }
    }
    Ok(())
}

/// Whether `flow` already completed for `docname` (guards `run_once` flows).
async fn already_ran(state: &IntegrationsContext, flow: &str, docname: &str) -> Result<bool> {
    let rows = store::list(
        state,
        AUTOMATION_TRIGGER_QUEUE,
        HashMap::from([
            ("flow".to_string(), Value::string(flow)),
            ("document_name".to_string(), Value::string(docname)),
            ("status".to_string(), Value::string("Completed")),
        ]),
    )
    .await?;
    Ok(!rows.is_empty())
}

/// Process one queued trigger to a terminal state regardless of outcome.
async fn process(state: &IntegrationsContext, queue_name: &str) -> Result<()> {
    let row = match store::get(state, AUTOMATION_TRIGGER_QUEUE, queue_name).await {
        Ok(row) => row,
        Err(e) => {
            tracing::warn!(queue_name, error = %e, "automation trigger vanished while queued");
            return Ok(());
        }
    };
    if str_field(&row, "status") != "Queued" {
        return Ok(()); // already terminal — a concurrent processor claimed it
    }
    let now = Utc::now().to_rfc3339();
    store::update(
        state,
        AUTOMATION_TRIGGER_QUEUE,
        queue_name,
        Value::from_json(serde_json::json!({
            "status": "Running",
            "started_at": now,
        })),
    )
    .await?;

    let doctype = str_field(&row, "document_type");
    let docname = str_field(&row, "document_name");
    // Re-read the document for template data; a deleted document yields the
    // empty field set (a queued run must not fail because the row vanished).
    let fields = store::get(state, &doctype, &docname)
        .await
        .map(|doc| fields_of(&doc))
        .unwrap_or_default();

    let flow_name = str_field(&row, "flow");
    let actions = store::get(state, AUTOMATION_FLOW, &flow_name)
        .await
        .map(|flow| parse_actions(&actions_text(flow.get("actions").unwrap_or(&Value::Null))))
        .unwrap_or_default();

    let result = run_actions(state, &actions, &doctype, &docname, &fields).await;
    let finished_at = Utc::now().to_rfc3339();
    match result {
        Ok(()) => {
            store::update(
                state,
                AUTOMATION_TRIGGER_QUEUE,
                queue_name,
                Value::from_json(serde_json::json!({
                    "status": "Completed",
                    "finished_at": finished_at,
                })),
            )
            .await?;
        }
        Err(e) => {
            tracing::warn!(queue_name, error = %e, "automation run failed");
            store::update(
                state,
                AUTOMATION_TRIGGER_QUEUE,
                queue_name,
                Value::from_json(serde_json::json!({
                    "status": "Failed",
                    "finished_at": finished_at,
                    "error": e.to_string(),
                })),
            )
            .await?;
        }
    }
    Ok(())
}

/// Evaluate the enabled `Automation Flow`s for `(doctype, event)` and run every
/// match. Returns how many triggers were enqueued.
///
/// The hooks call this on the document events; [`super::jobs::AutomationJob`]
/// exposes the same entry from a queue worker.
///
/// # Errors
///
/// Fails when the configured `Automation Flow` rows cannot be listed or a
/// trigger row cannot be enqueued; a condition that does not parse is logged
/// and skipped, never fatal.
pub async fn evaluate(
    state: &IntegrationsContext,
    doctype: &str,
    event: &str,
    docname: &str,
    fields: &ValueMap,
) -> Result<usize> {
    let rows = store::list(
        state,
        AUTOMATION_FLOW,
        HashMap::from([
            ("document_type".to_string(), Value::string(doctype)),
            ("event".to_string(), Value::string(event)),
        ]),
    )
    .await?;

    let mut queued = Vec::new();
    for row in rows {
        if !bool_field(&row, "enabled", true) {
            continue;
        }
        let name = str_field(&row, "name");
        if name.is_empty() {
            continue;
        }
        if bool_field(&row, "run_once", false) && already_ran(state, &name, docname).await? {
            continue;
        }
        let condition_text = str_field(&row, "condition");
        let matches = match condition::evaluate_optional(
            (!condition_text.is_empty()).then_some(condition_text.as_str()),
            fields,
        ) {
            Ok(true) => true,
            Ok(false) => false,
            Err(e) => {
                tracing::warn!(flow = %name, error = %e, "automation condition failed to evaluate; skipping");
                false
            }
        };
        if !matches {
            continue;
        }
        let queue_row = store::create(
            state,
            AUTOMATION_TRIGGER_QUEUE,
            Value::from_json(serde_json::json!({
                "flow": name,
                "document_type": doctype,
                "document_name": docname,
                "event": event,
                "status": "Queued",
            })),
        )
        .await?;
        if let Some(qname) = queue_row
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        {
            queued.push(qname);
        }
    }

    for qname in &queued {
        if let Err(e) = process(state, qname).await {
            tracing::warn!(queue_name = %qname, error = %e, "automation processing failed");
        }
    }
    Ok(queued.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_parse_arrays_and_single_objects() {
        let actions = parse_actions(
            r#"[
                {"type": "notification", "user": "ada@example.com", "title": "Hello", "message": "Hi {{ doc.title }}"},
                {"type": "assign", "assign_to": "bob@example.com"},
                {"type": "sms"}
            ]"#,
        );
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].kind, ActionKind::Notification);
        assert_eq!(actions[0].user.as_deref(), Some("ada@example.com"));
        assert_eq!(actions[1].kind, ActionKind::Assign);
        assert_eq!(actions[1].user.as_deref(), Some("bob@example.com"));
    }

    #[test]
    fn actions_garbage_and_empty_parse_to_nothing() {
        assert!(parse_actions("not json").is_empty());
        assert!(parse_actions("").is_empty());
        assert!(parse_actions("42").is_empty());
    }
}
