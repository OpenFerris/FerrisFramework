//! Reminders: notify a user when `remind_at` arrives.
//!
//! `run_due` walks the unsent `Reminder` rows whose `remind_at` has passed and
//! delivers each (`Notification`, `Email` or `Both`, per the row's `notify`
//! select), then marks it `sent` so a run never repeats it. The user's id is
//! also its mail address.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use ferris_core::{Result, Value};

use super::names::REMINDER;
use super::state::{EmailMessage, IntegrationsContext};
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

fn datetime_field(doc: &Value, key: &str) -> Option<DateTime<Utc>> {
    let raw = str_field(doc, key);
    DateTime::parse_from_rfc3339(&raw)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| naivetime(&raw).map(|naive| naive.and_utc()))
}

fn naivetime(raw: &str) -> Option<chrono::NaiveDateTime> {
    chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S").ok()
}

/// Deliver every due, unsent reminder. Returns how many were delivered.
///
/// # Errors
///
/// Fails when the `Reminder` rows cannot be listed; per-row delivery failures
/// (a failing mail transport) are logged and the row stays unsent for a retry.
pub async fn run_due(state: &IntegrationsContext, now: DateTime<Utc>) -> Result<usize> {
    let rows = store::list(state, REMINDER, HashMap::new()).await?;
    let mut delivered = 0;
    for row in rows {
        if bool_field(&row, "sent", false) {
            continue;
        }
        let Some(remind_at) = datetime_field(&row, "remind_at") else {
            continue;
        };
        if remind_at > now {
            continue;
        }
        let user = str_field(&row, "user");
        if user.is_empty() {
            continue;
        }
        let name = str_field(&row, "name");
        let doctype = str_field(&row, "document_type");
        let docname = str_field(&row, "document_name");
        let summary = {
            let text = str_field(&row, "reminder");
            let description = str_field(&row, "description");
            if description.is_empty() {
                text
            } else {
                format!("{text}: {description}")
            }
        };
        let notify = str_field(&row, "notify"); // "Email" | "Notification" | "Both"
        let route = format!("/app/{doctype}/{docname}");
        if !matches!(notify.as_str(), "Email" | "Notification" | "Both") {
            // A row the Desk let through with an empty/unknown channel would
            // otherwise be treated as "Notification" by elimination.
            tracing::warn!(name, %notify, "reminder has an unknown notify channel; skipping");
            continue;
        }

        let mut failed = false;

        if notify == "Email" || notify == "Both" {
            let subject = if docname.is_empty() {
                format!("Reminder: {summary}")
            } else {
                format!("Reminder: {doctype} {docname}")
            };
            let message = EmailMessage::new(user.clone(), subject, summary.clone());
            if let Err(e) = state.mailer.send(&message) {
                tracing::warn!(user, error = %e, "reminder email failed; leaving it unsent");
                failed = true;
            }
        }
        if notify != "Email" {
            let title = if docname.is_empty() {
                "Reminder".to_string()
            } else {
                format!("Reminder: {doctype} {docname}")
            };
            if let Err(e) = state.notify(&user, &title, &summary, &route).await {
                tracing::warn!(user, error = %e, "reminder notification failed; leaving it unsent");
                failed = true;
            }
        }

        if failed {
            // Do **not** flip `sent`: a failed delivery must be retried by the
            // next sweep, and it is not a delivery to count. The row already
            // has `sent = false`, so there is nothing to write.
            continue;
        }

        if let Err(e) = store::update(
            state,
            REMINDER,
            &name,
            Value::from_json(serde_json::json!({ "sent": true })),
        )
        .await
        {
            // The message went out but the flag write failed: report it and
            // leave the row unsent, so the next sweep resends (at-least-once).
            tracing::warn!(name, error = %e, "reminder failed to mark itself sent");
        }
        delivered += 1;
    }
    Ok(delivered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rfc3339_and_sqlish_datetimes() {
        let doc = Value::from_json(serde_json::json!({
            "remind_at": "2026-10-01T09:00:00Z"
        }));
        assert!(datetime_field(&doc, "remind_at").is_some());

        let doc2 = Value::from_json(serde_json::json!({
            "remind_at": "2026-10-01 09:00:00"
        }));
        assert!(datetime_field(&doc2, "remind_at").is_some());
    }
}

#[cfg(all(test, feature = "runtime"))]
mod delivery_tests {
    use std::sync::{Arc, Mutex};

    use chrono::{Duration as ChronoDuration, Utc};
    use ferris_core::{EmailError, Error, Result, Value};

    use super::super::names::REMINDER;
    use super::super::state::{EmailMessage, Mailer};
    use super::super::store;
    use super::super::testing::{exclusive, test_context};
    use super::{bool_field, run_due};

    struct FailingMailer;

    impl Mailer for FailingMailer {
        fn send(&self, _message: &EmailMessage) -> Result<()> {
            Err(Error::Email(EmailError::Smtp("transport down".to_string())))
        }
    }

    #[derive(Default)]
    struct CountingMailer(Arc<Mutex<usize>>);

    impl Mailer for CountingMailer {
        fn send(&self, _message: &EmailMessage) -> Result<()> {
            *self.0.lock().unwrap() += 1;
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_failed_reminder_stays_unsent_and_is_retried() {
        let _guard = exclusive().await;
        let ctx = test_context().await;
        let now = Utc::now();
        let past = (now - ChronoDuration::minutes(5)).to_rfc3339();

        store::create(
            &ctx,
            REMINDER,
            Value::from_json(serde_json::json!({
                "reminder": "Call the client",
                "document_type": "Contact",
                "document_name": "some-contact",
                "remind_at": past,
                "notify": "Email",
                "user": "ada@example.com",
                "sent": false,
            })),
        )
        .await
        .expect("reminder row");

        // A failing transport must not mark the reminder sent nor count it.
        let failing = ctx.clone().with_mailer(Arc::new(FailingMailer));
        let delivered = run_due(&failing, now).await.expect("run_due");
        assert_eq!(delivered, 0, "a failed send is not a delivery");

        let rows = store::list(&failing, REMINDER, std::collections::HashMap::new())
            .await
            .expect("reminders");
        assert!(
            !bool_field(&rows[0], "sent", false),
            "a failed reminder must stay unsent for the next sweep"
        );

        // The next sweep with a working transport delivers and records it.
        let mailer = Arc::new(CountingMailer::default());
        let sent = mailer.0.clone();
        let retry = ctx.with_mailer(mailer);
        let delivered = run_due(&retry, now).await.expect("retry");
        assert_eq!(delivered, 1);
        assert_eq!(*sent.lock().unwrap(), 1, "the retry must send exactly once");

        let rows = store::list(&retry, REMINDER, std::collections::HashMap::new())
            .await
            .expect("reminders");
        assert!(
            bool_field(&rows[0], "sent", false),
            "a delivered reminder is sent"
        );
    }
}
