//! Tests for the email hub, its delivery records and the IMAP poller wiring.

use ferris_core::Value;
use ferris_document::{DocumentEngine, EngineContext, ListQuery};
use ferris_email::{Email, EmailJob, MailLog};
use ferris_jobs::JobPriority;
use ferris_schema::DoctypeMeta;

use super::*;
use crate::roles::ServerRoles;

fn settings_with_email(enabled: bool) -> ServerSettings {
    let mut config = ferris_core::Config::default();
    config.features.email = enabled;
    ServerSettings::new(
        "127.0.0.1".to_string(),
        0,
        ServerRoles::all(),
        ferris_core::DatabaseConfig::default(),
        config,
    )
}

fn account(imap_host: Option<&str>) -> ferris_core::config::EmailAccountConfig {
    ferris_core::config::EmailAccountConfig {
        name: "soporte".to_string(),
        email: "soporte@example.com".to_string(),
        smtp_host: "smtp.example.com".to_string(),
        smtp_port: 587,
        smtp_username: "soporte".to_string(),
        smtp_password: "smtp-secret".to_string(),
        smtp_tls: false,
        smtp_starttls: true,
        imap_host: imap_host.map(str::to_string),
        imap_port: None,
        imap_username: None,
        imap_password: Some("imap-secret".to_string()),
        imap_tls: true,
        imap_starttls: false,
    }
}

fn settings_with_account(
    account: ferris_core::config::EmailAccountConfig,
    enabled: bool,
) -> ServerSettings {
    let mut settings = settings_with_email(enabled);
    settings.config.email.accounts = vec![account];
    settings
}

/// `[features] email = false` must not leave a hub built behind the server's
/// back: a caller that ignores the flag gets a loud error instead of a working
/// email subsystem the operator turned off.
#[tokio::test]
async fn an_email_hub_is_not_built_when_the_feature_is_disabled() {
    let settings = settings_with_email(false);
    let pool = ferris_db::memory_db().await.unwrap();
    let Err(err) = build_email_hub(&settings, &pool).await else {
        panic!("the email hub must not be built while `email` is disabled");
    };
    assert!(err.to_string().contains("email"), "{err}");
}

#[tokio::test]
async fn an_account_without_imap_host_starts_no_poller() {
    let settings = settings_with_account(account(None), true);
    let pool = ferris_db::memory_db().await.unwrap();
    assert_eq!(start_imap_pollers(&settings, &pool).active(), 0);
}

#[tokio::test]
async fn an_account_with_imap_starts_one_poller() {
    let settings = settings_with_account(account(Some("127.0.0.1")), true);
    let pool = ferris_db::memory_db().await.unwrap();
    let pollers = start_imap_pollers(&settings, &pool);
    assert_eq!(pollers.active(), 1);
    // Dropping aborts the task; the unreachable TLS endpoint is never required
    // to answer for this assertion.
}

#[tokio::test]
async fn imap_is_not_started_when_the_feature_is_disabled() {
    let settings = settings_with_account(account(Some("127.0.0.1")), false);
    let pool = ferris_db::memory_db().await.unwrap();
    assert_eq!(start_imap_pollers(&settings, &pool).active(), 0);
}

#[test]
fn imap_defaults_follow_the_tls_mode_and_the_account_address() {
    let implicit = imap_config(&account(Some("imap.example.com"))).expect("configured");
    assert_eq!(implicit.port, 993);
    assert!(implicit.use_tls);
    assert!(!implicit.use_starttls);
    // No `imap_username`: the mailbox address is the login.
    assert_eq!(implicit.username, "soporte@example.com");

    let mut starttls = account(Some("imap.example.com"));
    starttls.imap_tls = false;
    starttls.imap_starttls = true;
    assert_eq!(imap_config(&starttls).expect("configured").port, 143);

    // Explicit port wins over the mode's convention.
    let mut explicit = account(Some("imap.example.com"));
    explicit.imap_port = Some(1993);
    assert_eq!(imap_config(&explicit).expect("configured").port, 1993);

    assert!(imap_config(&account(None)).is_none());
    assert!(imap_config(&account(Some("   "))).is_none());
}

#[test]
fn an_imap_account_left_in_clear_text_is_rejected() {
    let mut clear = account(Some("imap.example.com"));
    clear.imap_tls = false;
    clear.imap_starttls = false;
    let config = imap_config(&clear).expect("mapped");
    assert!(
        ImapClient::new(config).is_err(),
        "inbound must not authenticate in clear text"
    );
}

// ---------------------------------------------------------------------------
// Delivery records: `Email Queue`, `Communication` and the group list.
// ---------------------------------------------------------------------------

fn job(to: &str, subject: &str) -> EmailJob {
    EmailJob {
        account: "soporte".to_string(),
        from: "soporte@example.com".to_string(),
        to: vec![to.to_string()],
        cc: vec![],
        bcc: vec![],
        subject: subject.to_string(),
        text: Some("Body".to_string()),
        html: None,
        template: None,
        template_context: None,
        reply_to: None,
        in_reply_to: None,
        references: vec![],
        attachments: vec![],
        priority: JobPriority::Normal,
        scheduled_at: None,
    }
}

fn engine_ctx(doctype: &str) -> EngineContext<'static> {
    // Leaked on purpose: the context only lives for one assertion and the
    // engine borrows the meta for the duration of the call.
    let meta: &'static DoctypeMeta = Box::leak(Box::new(
        ferris_schema::get_doctype(doctype).expect("registered"),
    ));
    let mut ctx = EngineContext::new(doctype, "");
    ctx.meta = Some(meta);
    let mut user = ferris_core::User::system_user("test@ferris.local".into(), "Test".into());
    user.roles.push("System Manager".into());
    ctx.user = Some(user);
    ctx
}

async fn rows(db: &Db, doctype: &str) -> Vec<Value> {
    DocumentEngine::new(db.clone())
        .list(
            &engine_ctx(doctype),
            &ListQuery {
                limit: Some(100),
                ..Default::default()
            },
        )
        .await
        .expect("list")
        .0
}

async fn create_group(db: &Db, members: &str, unsubscribed: &str) -> String {
    let created = DocumentEngine::new(db.clone())
        .create(
            &engine_ctx("Email Group"),
            Value::from_json(serde_json::json!({
                "group_name": "Newsletter",
                "members": members,
                "unsubscribed": unsubscribed,
            })),
        )
        .await
        .expect("group");
    created
        .get("name")
        .and_then(Value::as_str)
        .expect("name")
        .to_string()
}

/// The queue creates the row; the worker settles that same row, and a delivery
/// appends the `Sent` communication. A row is never duplicated.
#[tokio::test]
async fn a_sent_email_settles_its_own_row_and_logs_the_communication() {
    let db = ferris_db::memory_db().await.expect("db");
    let log = DbMailLog::new(db.clone());
    let email = job("ada@example.com", "Hello");

    let row = log.queued(&email).await.expect("queued row");
    assert_eq!(rows(&db, "Email Queue").await.len(), 1);
    log.sent(Some(&row), &email, "<id@example.com>", 1)
        .await
        .expect("settle");

    let queue = rows(&db, "Email Queue").await;
    assert_eq!(
        queue.len(),
        1,
        "the worker marks the row, it does not add one"
    );
    let entry = &queue[0];
    let field = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or_default();
    assert_eq!(
        entry.get("name").and_then(Value::as_str),
        Some(row.as_str())
    );
    assert_eq!(field("status"), "Sent");
    assert_eq!(field("recipients"), "ada@example.com");
    assert_eq!(field("message_id"), "<id@example.com>");
    assert_eq!(entry.get("attempts").and_then(Value::as_i64), Some(1));
    assert!(!field("sent_at").is_empty(), "sent_at is stamped");
    let communication = field("communication").to_string();

    let log_rows = rows(&db, "Communication").await;
    assert_eq!(log_rows.len(), 1);
    let logged = &log_rows[0];
    assert_eq!(
        logged.get("name").and_then(Value::as_str),
        Some(communication.as_str())
    );
    assert_eq!(
        logged.get("sent_or_received").and_then(Value::as_str),
        Some("Sent")
    );
    assert_eq!(
        logged.get("message_id").and_then(Value::as_str),
        Some("<id@example.com>")
    );
    assert_eq!(
        logged.get("reference_name").and_then(Value::as_str),
        Some(row.as_str())
    );

    // A job queued by a producer that did not log it still gets exactly one row.
    log.sent(None, &job("bob@example.com", "Unlogged"), "", 1)
        .await
        .expect("settle");
    assert_eq!(rows(&db, "Email Queue").await.len(), 2);
    assert_eq!(rows(&db, "Communication").await.len(), 2);
}

/// A failed delivery keeps the cause on the row. While the job queue will
/// retry, the row stays `Queued`; the last attempt makes the failure final.
#[tokio::test]
async fn a_failed_delivery_records_its_error() {
    let db = ferris_db::memory_db().await.expect("db");
    let log = DbMailLog::new(db.clone());
    let email = job("ada@example.com", "Boom");
    let row = log.queued(&email).await.expect("queued row");

    log.failed(Some(&row), &email, "connection refused", 1, true)
        .await
        .expect("retryable failure");
    let entry = &rows(&db, "Email Queue").await[0];
    assert_eq!(entry.get("status").and_then(Value::as_str), Some("Queued"));
    assert_eq!(entry.get("attempts").and_then(Value::as_i64), Some(1));
    assert_eq!(
        entry.get("error").and_then(Value::as_str),
        Some("connection refused")
    );

    log.failed(Some(&row), &email, "connection refused", 3, false)
        .await
        .expect("final failure");
    let entry = &rows(&db, "Email Queue").await[0];
    assert_eq!(entry.get("status").and_then(Value::as_str), Some("Failed"));
    assert_eq!(entry.get("attempts").and_then(Value::as_i64), Some(3));
    assert!(
        rows(&db, "Communication").await.is_empty(),
        "nothing was sent"
    );
}

/// A group send is one message per member, minus the opt-outs, with the opt-out
/// instructions in the body.
#[tokio::test]
async fn a_group_send_reaches_members_and_skips_the_opt_outs() {
    let db = ferris_db::memory_db().await.expect("db");
    let settings = settings_with_account(account(None), true);
    let queue = JobQueue::new_db(
        QueueConfig {
            name: EMAIL_QUEUE.to_string(),
            retry_backoff_base_secs: 1,
            dead_letter_queue: "dead".to_string(),
        },
        db.clone(),
    );
    queue.init().await.expect("queue");
    let mut accounts = EmailAccountManager::new();
    accounts.add_account(email_account(&settings.config.email.accounts[0]).expect("account"));
    let hub = EmailHub {
        queue: Arc::new(queue),
        accounts: Arc::new(accounts),
        default_account: "soporte".to_string(),
        log: Arc::new(DbMailLog::new(db.clone())),
        _pollers: ImapPollers::default(),
    };

    let group = create_group(
        &db,
        "ada@example.com\nbob@example.com\ncy@example.com",
        "cy@example.com",
    )
    .await;
    let message =
        Email::new(String::new(), vec![], "Weekly".to_string()).with_text("The news".to_string());
    assert_eq!(
        hub.send_to_group(&group, message.clone())
            .await
            .expect("send"),
        2
    );

    let queue = rows(&db, "Email Queue").await;
    assert_eq!(queue.len(), 2);
    let mut recipients: Vec<_> = queue
        .iter()
        .filter_map(|entry| entry.get("recipients").and_then(Value::as_str))
        .collect();
    recipients.sort_unstable();
    assert_eq!(recipients, vec!["ada@example.com", "bob@example.com"]);
    assert!(
        queue.iter().all(|entry| entry
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|body| body.contains("email group"))),
        "every group message carries the opt-out instructions"
    );

    assert!(
        hub.unsubscribe(&group, "ada@example.com")
            .await
            .expect("opt out")
    );
    assert!(
        !hub.unsubscribe(&group, "ada@example.com")
            .await
            .expect("again"),
        "the second opt-out is a no-op"
    );
    assert_eq!(hub.send_to_group(&group, message).await.expect("send"), 1);
    assert_eq!(rows(&db, "Email Queue").await.len(), 3);
}

/// `Email Settings.default_outgoing_account` decides which account sends; an
/// unknown or empty value keeps the historical first-account default.
#[tokio::test]
async fn email_settings_chooses_the_default_outgoing_account() {
    let db = ferris_db::memory_db().await.expect("db");
    ferris_email::doctypes::register_doctypes();
    let mut settings = settings_with_account(account(None), true);
    let mut second = account(None);
    second.name = "facturas".to_string();
    second.email = "facturas@example.com".to_string();
    settings.config.email.accounts.push(second);

    let engine = DocumentEngine::new(db.clone());
    for (configured, expected) in [("facturas", "facturas"), ("nadie", "soporte")] {
        engine
            .save_single(
                &engine_ctx("Email Settings"),
                Value::from_json(serde_json::json!({
                    "default_outgoing_account": configured,
                })),
            )
            .await
            .expect("save settings");
        assert_eq!(
            settings_default_account(&db).await.as_deref(),
            Some(configured),
            "the reader returns what the Single declares"
        );
        let hub = build_email_hub(&settings, &db).await.expect("hub");
        assert_eq!(hub.default_account, expected, "configured `{configured}`");
    }
}
