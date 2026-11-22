//! End-to-end flows: document events driving webhooks, automation, assignment,
//! milestones, push; the scheduled sweeps for auto repeat/reminders/auto email
//! reports. Every test uses its own in-memory database and serialises through
//! [`exclusive`](super::testing::exclusive) — the process-wide hook registry is
//! global state, so only one test may install hooks / write documents at a
//! time (a stale hook bound to another test's database would dispatch
//! nowhere, but would corrupt assertions).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{Duration as ChronoDuration, NaiveDate, Utc};
use ferris_core::{Result, Value};

use super::names::{
    ASSIGNMENT_RULE, AUTO_EMAIL_REPORT, AUTO_REPEAT, AUTOMATION_FLOW, AUTOMATION_TRIGGER_QUEUE,
    GOOGLE_CONTACTS, MILESTONE, NOTIFICATION, PUSH_NOTIFICATION, REMINDER, WEBHOOK,
};
use super::state::{EmailMessage, IntegrationsContext, Mailer};
use super::testing::{TestServer, exclusive, test_context};
use super::{auto_email_report, auto_repeat, hooks, push, reminder, store};

fn str_f(doc: &Value, key: &str) -> String {
    doc.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn bool_f(doc: &Value, key: &str) -> bool {
    doc.get(key).and_then(Value::as_bool).unwrap_or(false)
}

async fn create_contact(ctx: &IntegrationsContext, name: &str, email: &str) -> String {
    let doc = store::create(
        ctx,
        GOOGLE_CONTACTS,
        Value::from_json(serde_json::json!({
            "contact_name": name,
            "email": email,
        })),
    )
    .await
    .expect("contact created");
    str_f(&doc, "name")
}

/// A [`Mailer`] that records every message (and always succeeds), so a test
/// can assert on what a reminder or report asked the transport to send.
#[derive(Clone, Default)]
struct RecordingMailer(Arc<Mutex<Vec<EmailMessage>>>);

impl RecordingMailer {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    fn sent(&self) -> Vec<EmailMessage> {
        self.0.lock().unwrap().clone()
    }
}

impl Mailer for RecordingMailer {
    fn send(&self, message: &EmailMessage) -> Result<()> {
        self.0.lock().unwrap().push(message.clone());
        Ok(())
    }
}

#[tokio::test]
async fn webhook_dispatches_a_signed_payload_on_after_insert() {
    let _exclusive = exclusive().await;
    let server = TestServer::start(200, "{}");
    let ctx = test_context().await;
    hooks::install(ctx.clone());

    store::create(
        &ctx,
        WEBHOOK,
        Value::from_json(serde_json::json!({
            "webhook_doctype": GOOGLE_CONTACTS,
            "webhook_docevent": "after_insert",
            "request_url": format!("{}/hook", server.url),
            "request_method": "POST",
            "request_body": r#"{"email": "{{ doc.email }}", "doc": "{{ doctype }}"}"#,
            "webhook_secret": "whsec",
            "enabled": true,
        })),
    )
    .await
    .expect("webhook row");

    create_contact(&ctx, "Ada Lovelace", "ada@example.com").await;

    let requests = server.received();
    assert_eq!(requests.len(), 1, "one dispatch for the configured event");
    let req = &requests[0];
    assert_eq!(req.method, "POST");
    assert!(req.path.starts_with("/hook"), "path was {}", req.path);
    let parsed: serde_json::Value = serde_json::from_str(&req.body_text()).expect("json body");
    assert_eq!(parsed["email"], "ada@example.com");
    assert_eq!(parsed["doc"], GOOGLE_CONTACTS);
    assert_eq!(
        req.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    let signature = req
        .headers
        .get("x-ferris-webhook-signature")
        .expect("HMAC header present");
    assert_eq!(signature.len(), 64, "sha256 hex signature");
    // The point of H3: the signature is computed with the *plaintext* secret,
    // which used to read back as the `Password` mask. Compute it independently
    // from the body the endpoint received.
    let expected = {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"whsec").expect("hmac key");
        mac.update(&req.body);
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    assert_eq!(signature, &expected);
}

#[tokio::test]
async fn automation_flow_runs_actions_and_records_completion() {
    let _exclusive = exclusive().await;
    let ctx = test_context().await;
    hooks::install(ctx.clone());

    store::create(
        &ctx,
        AUTOMATION_FLOW,
        Value::from_json(serde_json::json!({
            "flow_name": "New contact alert",
            "document_type": GOOGLE_CONTACTS,
            "event": "after_insert",
            "condition": "",
            "actions": [
                {"type": "notification", "user": "ada@example.com",
                 "title": "New contact", "message": "{{ doc.contact_name }} added"}
            ],
            "run_once": false,
            "enabled": true,
        })),
    )
    .await
    .expect("automation flow row");

    let docname = create_contact(&ctx, "Grace Hopper", "grace@example.com").await;

    let queue = store::list(&ctx, AUTOMATION_TRIGGER_QUEUE, HashMap::new())
        .await
        .expect("trigger queue");
    assert_eq!(queue.len(), 1, "after_insert matches exactly once");
    assert_eq!(str_f(&queue[0], "status"), "Completed");
    assert_eq!(str_f(&queue[0], "document_name"), docname);

    let notifications = store::list(&ctx, NOTIFICATION, HashMap::new())
        .await
        .expect("notifications");
    let target = notifications
        .iter()
        .find(|n| str_f(n, "user") == "ada@example.com")
        .expect("notification raised for the flow's user");
    assert_eq!(str_f(target, "title"), "New contact");
    assert!(str_f(target, "description").contains("Grace Hopper"));
    assert_eq!(
        str_f(target, "route"),
        format!("/app/{GOOGLE_CONTACTS}/{docname}")
    );
}

#[tokio::test]
async fn assignment_rule_notifies_the_assignee_on_insert() {
    let _exclusive = exclusive().await;
    let ctx = test_context().await;
    hooks::install(ctx.clone());

    store::create(
        &ctx,
        ASSIGNMENT_RULE,
        Value::from_json(serde_json::json!({
            "rule_name": "Contacts to Ada",
            "document_type": GOOGLE_CONTACTS,
            "assign_to": "ada@example.com",
            "priority": 5,
            "disabled": false,
        })),
    )
    .await
    .expect("assignment rule row");

    let docname = create_contact(&ctx, "Grace Hopper", "grace@example.com").await;

    let notifications = store::list(&ctx, NOTIFICATION, HashMap::new())
        .await
        .expect("notifications");
    let assigned = notifications
        .iter()
        .find(|n| {
            str_f(n, "user") == "ada@example.com" && str_f(n, "title").starts_with("Assigned:")
        })
        .expect("assignment notification");
    assert_eq!(
        str_f(assigned, "route"),
        format!("/app/{GOOGLE_CONTACTS}/{docname}")
    );
}

#[tokio::test]
async fn milestone_reaches_achieved_when_the_tracked_field_matches() {
    let _exclusive = exclusive().await;
    let ctx = test_context().await;
    hooks::install(ctx.clone());

    let docname = create_contact(&ctx, "Ada Lovelace", "ada@example.com").await;

    store::create(
        &ctx,
        MILESTONE,
        Value::from_json(serde_json::json!({
            "milestone": "Onboarded",
            "document_type": GOOGLE_CONTACTS,
            "document_name": docname,
            "track_field": "email",
            "value": "ada@example.com",
            "achieved": false,
        })),
    )
    .await
    .expect("milestone row");

    // Touching the document fires `OnUpdate`, which runs the milestone check
    // against the stored field value.
    store::update(
        &ctx,
        GOOGLE_CONTACTS,
        &docname,
        Value::from_json(serde_json::json!({ "phone": "555-0100" })),
    )
    .await
    .expect("update");

    let milestones = store::list(&ctx, MILESTONE, HashMap::new())
        .await
        .expect("milestones");
    assert_eq!(milestones.len(), 1);
    assert!(
        bool_f(&milestones[0], "achieved"),
        "milestone should be achieved"
    );
}

#[tokio::test]
async fn auto_repeat_clones_the_reference_once_a_day() {
    let _exclusive = exclusive().await;
    let ctx = test_context().await;
    hooks::install(ctx.clone());
    let today = NaiveDate::from_ymd_opt(2026, 10, 10).unwrap();

    let docname = create_contact(&ctx, "Original", "o@example.com").await;
    store::create(
        &ctx,
        AUTO_REPEAT,
        Value::from_json(serde_json::json!({
            "reference_doctype": GOOGLE_CONTACTS,
            "reference_document": docname,
            "frequency": "Daily",
            "start_date": "2026-10-09",
            "disabled": false,
        })),
    )
    .await
    .expect("auto repeat row");

    let created = auto_repeat::run_due(&ctx, today).await.expect("run_due");
    assert_eq!(created, 1);

    let contacts = store::list(&ctx, GOOGLE_CONTACTS, HashMap::new())
        .await
        .expect("contacts");
    assert_eq!(contacts.len(), 2, "original + one clone");
    // The clone keeps the contact_name but must get a fresh document id: only
    // the original row carries the reference id.
    let same_id = contacts
        .iter()
        .filter(|c| str_f(c, "name") == docname)
        .count();
    assert_eq!(same_id, 1, "original keeps its id, clone gets a new one");

    let repeats = store::list(&ctx, AUTO_REPEAT, HashMap::new())
        .await
        .expect("auto repeat rows");
    assert_eq!(str_f(&repeats[0], "next_schedule_date"), "2026-10-11");

    // Same day again: the roll-forward made it not-due, so no second clone.
    let created = auto_repeat::run_due(&ctx, today).await.expect("second run");
    assert_eq!(created, 0);
}

#[tokio::test]
async fn reminder_delivers_notification_and_email_then_marks_sent() {
    let _exclusive = exclusive().await;
    let mailer = Arc::new(RecordingMailer::new());
    let ctx = test_context().await.with_mailer(mailer.clone());
    hooks::install(ctx.clone());
    let now = Utc::now();
    let past = (now - ChronoDuration::minutes(5)).to_rfc3339();

    store::create(
        &ctx,
        REMINDER,
        Value::from_json(serde_json::json!({
            "reminder": "Call the client",
            "document_type": GOOGLE_CONTACTS,
            "document_name": "some-contact",
            "remind_at": past,
            "notify": "Both",
            "user": "ada@example.com",
            "sent": false,
        })),
    )
    .await
    .expect("reminder row");

    let delivered = reminder::run_due(&ctx, now).await.expect("run_due");
    assert_eq!(delivered, 1);

    let notifications = store::list(&ctx, NOTIFICATION, HashMap::new())
        .await
        .expect("notifications");
    assert!(
        notifications
            .iter()
            .any(|n| str_f(n, "user") == "ada@example.com")
    );

    let sent = mailer.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, vec!["ada@example.com".to_string()]);
    assert!(sent[0].subject.contains("Reminder"));

    let reminders = store::list(&ctx, REMINDER, HashMap::new())
        .await
        .expect("reminders");
    assert!(
        bool_f(&reminders[0], "sent"),
        "reminder should be marked sent"
    );
}

#[tokio::test]
async fn auto_email_report_sends_once_per_period() {
    let _exclusive = exclusive().await;
    let mailer = Arc::new(RecordingMailer::new());
    let ctx = test_context().await.with_mailer(mailer.clone());
    hooks::install(ctx.clone());

    store::create(
        &ctx,
        AUTO_EMAIL_REPORT,
        Value::from_json(serde_json::json!({
            "report": "Sales summary",
            "email_to": "ops@example.com, finance@example.com",
            "frequency": "Daily",
            "enabled": true,
        })),
    )
    .await
    .expect("auto email report row");

    let now = Utc::now();
    let sent = auto_email_report::run_due(&ctx, now)
        .await
        .expect("run_due");
    assert_eq!(sent, 1);

    let messages = mailer.sent();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].to,
        vec![
            "ops@example.com".to_string(),
            "finance@example.com".to_string()
        ]
    );
    assert!(messages[0].text.contains("Report: Sales summary"));

    // Same period: `last_sent_on` is set, no duplicate delivery.
    let sent_again = auto_email_report::run_due(&ctx, now)
        .await
        .expect("second run");
    assert_eq!(sent_again, 0);
    assert_eq!(mailer.sent().len(), 1);

    let rows = store::list(&ctx, AUTO_EMAIL_REPORT, HashMap::new())
        .await
        .expect("reports");
    assert!(
        !str_f(&rows[0], "last_sent_on").is_empty(),
        "sends are bookkept"
    );
}

#[tokio::test]
async fn push_notification_delivers_as_an_in_app_notification() {
    let _exclusive = exclusive().await;
    let ctx = test_context().await;
    hooks::install(ctx.clone());

    store::create(
        &ctx,
        PUSH_NOTIFICATION,
        Value::from_json(serde_json::json!({
            "title": "Deploy failed",
            "body": "The nightly build broke.",
            "user": "ada@example.com",
            "document_type": "",
            "document_name": "",
            "status": "Queued",
        })),
    )
    .await
    .expect("push row");

    // Delivery is sweep-driven (a hook cannot persist `Sent` on its own
    // DocType — the engine re-persists the document after the hook batch), so
    // the pending sweep performs the raise and the terminal transition.
    let delivered = push::deliver_pending(&ctx).await.expect("delivery sweep");
    assert_eq!(delivered, 1);

    let rows = store::list(&ctx, PUSH_NOTIFICATION, HashMap::new())
        .await
        .expect("push rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(str_f(&rows[0], "status"), "Sent");
    assert!(!str_f(&rows[0], "sent_at").is_empty());

    let notifications = store::list(&ctx, NOTIFICATION, HashMap::new())
        .await
        .expect("notifications");
    assert!(notifications.iter().any(|n| {
        str_f(n, "user") == "ada@example.com" && str_f(n, "title") == "Deploy failed"
    }));
}
