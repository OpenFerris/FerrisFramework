//! Push notification delivery.
//!
//! A `Push Notification` row is the SDK-shaped input (title, body, target
//! user); [`deliver`] turns a `Queued` row into an in-app `Notification` and
//! marks it `Sent`, or `Failed` with the error and a retry schedule. The host
//! runs the [`deliver_pending`] sweep (or the [`super::jobs::PushDeliveryJob`]
//! handler) every minute; a row inserted in the meantime is picked up on the
//! next tick, so a crash between insert and delivery is recovered.
//!
//! **The transport is the framework's own in-app feed** — a `Notification` row
//! plus the process-wide publisher that the server bridges to SSE/socket.io.
//! There is no APNs/FCM device transport: a push here reaches a signed-in Desk
//! session, not a phone.
//!
//! # Retries
//!
//! A failed delivery is not terminal until [`PushRetryPolicy::max_attempts`]
//! is reached. Each failure increments `attempts`, records `error`, and sets
//! `next_attempt_at` with an exponential backoff; [`deliver_pending`] also
//! selects `Failed` rows whose schedule is due. The row is never left in a
//! `Sending` state, so a crash cannot strand it: it stays `Queued`/`Failed` and
//! the next sweep retries it (at-least-once delivery, not exactly-once).
//!
//! Delivery deliberately happens outside the insert hook: the engine
//! re-persists the triggering document after every hook batch, so a status
//! write made *from within* the `after_insert` hook for its own DocType would
//! be overwritten. Sweep-driven delivery sidesteps that — the write happens
//! after the save has fully landed.

use chrono::{DateTime, Utc};
use ferris_core::{Error, Result, ValidationError, Value};

use super::names::PUSH_NOTIFICATION;
use super::state::IntegrationsContext;
use super::store;

fn str_field(doc: &Value, key: &str) -> String {
    doc.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn int_field(doc: &Value, key: &str) -> i64 {
    doc.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// Retry budget for a push row whose delivery failed.
#[derive(Debug, Clone, Copy)]
pub struct PushRetryPolicy {
    /// Attempts after which the row stays `Failed` for inspection.
    pub max_attempts: u32,
    /// First backoff step; doubles per attempt, capped at one hour.
    pub base_delay_secs: u64,
}

impl Default for PushRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_delay_secs: 60,
        }
    }
}

impl PushRetryPolicy {
    /// Seconds to wait after the `attempt`-th failure (1 = first).
    #[must_use]
    pub fn delay_secs(&self, attempt: u32) -> u64 {
        let shift = attempt.saturating_sub(1).min(10);
        self.base_delay_secs
            .saturating_mul(1_u64 << shift)
            .min(3600)
    }
}

/// Whether a `Failed` row's `next_attempt_at` has arrived. An empty value is
/// due immediately (a row written before this field existed).
fn is_due(row: &Value) -> bool {
    let next = str_field(row, "next_attempt_at");
    if next.is_empty() {
        return true;
    }
    DateTime::parse_from_rfc3339(&next).map_or(true, |at| at <= Utc::now())
}

/// Deliver one `Push Notification` row with the default retry policy. Returns
/// `Ok(())` once the row reached a terminal state or is not eligible; a
/// missing/already-delivered row is a no-op.
///
/// # Errors
///
/// A delivery that fails records the failure on the row (attempt, error, next
/// attempt) and returns it, so the caller can surface how loud it was.
pub async fn deliver(state: &IntegrationsContext, name: &str) -> Result<()> {
    deliver_with_policy(state, name, PushRetryPolicy::default()).await
}

/// [`deliver`] with an explicit retry policy (tests use a zero backoff).
///
/// # Errors
///
/// See [`deliver`].
pub async fn deliver_with_policy(
    state: &IntegrationsContext,
    name: &str,
    policy: PushRetryPolicy,
) -> Result<()> {
    let row = match store::get(state, PUSH_NOTIFICATION, name).await {
        Ok(row) => row,
        Err(e) => {
            tracing::warn!(name, error = %e, "push notification row unavailable");
            return Ok(());
        }
    };

    let status = str_field(&row, "status");
    if status == "Sent" || (status != "Queued" && status != "Failed") {
        return Ok(());
    }
    let attempts = u32::try_from(int_field(&row, "attempts").max(0)).unwrap_or(u32::MAX);
    if attempts >= policy.max_attempts {
        return Ok(());
    }
    if status == "Failed" && !is_due(&row) {
        return Ok(());
    }

    let now = Utc::now();
    let user = str_field(&row, "user");
    let deliverable = async {
        if user.is_empty() {
            return Err(Error::Validation(ValidationError::Required(
                "Push Notification target user".to_string(),
            )));
        }
        let title = str_field(&row, "title");
        let body = str_field(&row, "body");
        let doctype = str_field(&row, "document_type");
        let docname = str_field(&row, "document_name");
        let route = if doctype.is_empty() {
            "/".to_string()
        } else {
            format!("/app/{doctype}/{docname}")
        };
        state.notify(&user, &title, &body, &route).await?;
        Ok(())
    };
    let result = deliverable.await;
    let next_attempts = attempts.saturating_add(1);
    match result {
        Ok(()) => {
            store::update(
                state,
                PUSH_NOTIFICATION,
                name,
                // No `next_attempt_at` on success: a Datetime field rejects
                // the empty string, and the terminal status makes the stale
                // schedule irrelevant.
                Value::from_json(serde_json::json!({
                    "status": "Sent",
                    "sent_at": now.to_rfc3339(),
                    "attempts": i64::from(next_attempts),
                    "error": "",
                })),
            )
            .await?;
        }
        Err(e) => {
            tracing::warn!(name, attempts = next_attempts, error = %e, "push delivery failed");
            let exhausted = next_attempts >= policy.max_attempts;
            let mut update = serde_json::json!({
                "status": "Failed",
                "attempts": i64::from(next_attempts),
                "error": e.to_string(),
            });
            if !exhausted {
                let due = now
                    + chrono::TimeDelta::try_seconds(
                        i64::try_from(policy.delay_secs(next_attempts)).unwrap_or(i64::MAX),
                    )
                    .unwrap_or_default();
                update["next_attempt_at"] = serde_json::Value::String(due.to_rfc3339());
            }
            store::update(state, PUSH_NOTIFICATION, name, Value::from_json(update)).await?;
            return Err(e);
        }
    }
    Ok(())
}

/// Re-deliver every eligible row: `Queued` rows and `Failed` rows whose backoff
/// has elapsed and whose attempt budget is not exhausted.
///
/// # Errors
///
/// Fails when the rows cannot be listed; per-row delivery failures are logged
/// and recorded on the row for the next tick.
pub async fn deliver_pending(state: &IntegrationsContext) -> Result<usize> {
    let policy = PushRetryPolicy::default();
    let mut rows = store::list(
        state,
        PUSH_NOTIFICATION,
        std::collections::HashMap::from([("status".to_string(), Value::string("Queued"))]),
    )
    .await?;
    let failed = store::list(
        state,
        PUSH_NOTIFICATION,
        std::collections::HashMap::from([("status".to_string(), Value::string("Failed"))]),
    )
    .await?;
    rows.extend(failed.into_iter().filter(|row| {
        let attempts = u32::try_from(int_field(row, "attempts").max(0)).unwrap_or(u32::MAX);
        attempts < policy.max_attempts && is_due(row)
    }));

    let mut delivered = 0;
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let Some(name) = row.get("name").and_then(|v| v.as_str()).map(str::to_string) else {
            continue;
        };
        if !seen.insert(name.clone()) {
            continue;
        }
        match deliver_with_policy(state, &name, policy).await {
            Ok(()) => delivered += 1,
            Err(e) => {
                tracing::warn!(name, error = %e, "push re-delivery failed");
            }
        }
    }
    Ok(delivered)
}

#[cfg(test)]
mod tests;
