//! The durable delivery record: `Email Queue`, `Communication` and the
//! `Email Group` subscriber list.
//!
//! The transport crate (`ferris-email`) knows how to send but not how to store a
//! document — that needs the document engine and this process's database. It
//! publishes [`ferris_email::MailLog`] and this module implements it, so a
//! message queued by the hub leaves a `Queued` row, the worker settles that same
//! row (`Sent`/`Failed`, never a second one) and a successful delivery appends
//! the `Sent` `Communication`.
//!
//! Every write goes through the engine, not SQL: the rows are ordinary
//! documents, so they validate, get a version row, fire hooks and obey the
//! `System Manager` permission the `DocType`s declare.

use std::sync::Arc;

use async_trait::async_trait;
use ferris_core::{Result, User, Value};
use ferris_db::Db;
use ferris_document::{DocumentEngine, EngineContext};
use ferris_email::{EmailJob, MailLog, QueuedEmailJob};
use ferris_jobs::{JobContext, JobHandler, JobPriority};
use ferris_schema::DoctypeMeta;
use serde_json::json;
use tracing::warn;

/// `DocType` names this module reads and writes.
const EMAIL_QUEUE: &str = "Email Queue";
const COMMUNICATION: &str = "Communication";
const EMAIL_GROUP: &str = "Email Group";
/// Install-wide email defaults (a Single).
pub const EMAIL_SETTINGS: &str = "Email Settings";

fn service_user() -> User {
    let mut user = User::system_user(
        "email-log@ferris.local".to_string(),
        "Email Log".to_string(),
    );
    // The DocTypes written here declare `System Manager`; one principal covers
    // the queue, the log and the group list.
    user.roles.push("System Manager".to_string());
    user
}

/// `To`, `Cc` and `Bcc`, comma separated: the recipient column of both rows.
fn recipients(job: &EmailJob) -> String {
    job.to
        .iter()
        .chain(&job.cc)
        .chain(&job.bcc)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ")
}

/// The body the log keeps: the plain text, else the HTML.
fn body(job: &EmailJob) -> String {
    job.text
        .clone()
        .or_else(|| job.html.clone())
        .unwrap_or_default()
}

/// Non-empty, trimmed lines of a member or opt-out field.
fn lines(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// RFC 3339 in UTC, the format a `Datetime` field validates.
fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

/// [`MailLog`] backed by the document engine.
#[derive(Clone)]
pub struct DbMailLog {
    db: Db,
    user: User,
}

impl DbMailLog {
    /// Build the log and register the crate's `DocType`s (idempotent), so a hub
    /// built before the email module's boot can still write its first row.
    #[must_use]
    pub fn new(db: Db) -> Self {
        ferris_email::doctypes::register_doctypes();
        Self {
            db,
            user: service_user(),
        }
    }

    fn engine(&self) -> DocumentEngine {
        DocumentEngine::new(self.db.clone())
    }

    fn ctx<'a>(&self, doctype: &str, meta: Option<&'a DoctypeMeta>) -> EngineContext<'a> {
        let mut ctx = EngineContext::new(doctype, "");
        if let Some(meta) = meta {
            ctx.meta = Some(meta);
        }
        ctx.user = Some(self.user.clone());
        ctx
    }

    async fn create(&self, doctype: &str, data: serde_json::Value) -> Result<String> {
        let meta = ferris_schema::get_doctype(doctype);
        let ctx = self.ctx(doctype, meta.as_deref());
        let created = self.engine().create(&ctx, Value::from_json(data)).await?;
        created
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                ferris_core::Error::Validation(ferris_core::ValidationError::Required(format!(
                    "{doctype} name"
                )))
            })
    }

    async fn update(&self, doctype: &str, name: &str, data: serde_json::Value) -> Result<()> {
        let meta = ferris_schema::get_doctype(doctype);
        let ctx = self.ctx(doctype, meta.as_deref());
        self.engine()
            .update(&ctx, name, Value::from_json(data))
            .await?;
        Ok(())
    }

    /// The row the queue created, or a fresh one when the job was queued by a
    /// producer that did not log it.
    async fn ensure_row(&self, row: Option<&str>, job: &EmailJob) -> Result<String> {
        match row.filter(|name| !name.is_empty()) {
            Some(name) => Ok(name.to_string()),
            None => self.queued(job).await,
        }
    }

    /// Append the `Sent` `Communication` and return its name.
    async fn log_communication(
        &self,
        job: &EmailJob,
        queue_row: &str,
        message_id: &str,
    ) -> Result<String> {
        // Name and type only: the bytes were streamed by the transport and are
        // not this row's business.
        let attachments = job
            .attachments
            .iter()
            .map(|attachment| {
                attachment.filename.as_ref().map_or_else(
                    || format!("({})", attachment.content_type),
                    |name| format!("{name} ({})", attachment.content_type),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.create(
            COMMUNICATION,
            json!({
                "subject": job.subject,
                "sender": job.from,
                "recipients": recipients(job),
                "content": body(job),
                "sent_or_received": "Sent",
                "communication_type": "Email",
                "status": "Open",
                "email_account": job.account,
                "message_id": message_id,
                "in_reply_to": job.in_reply_to.clone().unwrap_or_default(),
                "attachments": attachments,
                "reference_doctype": EMAIL_QUEUE,
                "reference_name": queue_row,
            }),
        )
        .await
    }

    /// Addresses a group send may reach: members minus opt-outs, deduplicated
    /// case-insensitively and in list order.
    ///
    /// # Errors
    ///
    /// The group does not exist, or the engine cannot read it.
    pub async fn group_recipients(&self, group: &str) -> Result<Vec<String>> {
        let meta = ferris_schema::get_doctype(EMAIL_GROUP);
        let ctx = self.ctx(EMAIL_GROUP, meta.as_deref());
        let doc = self.engine().get(&ctx, group).await?;
        let unsubscribed = lines(doc.get("unsubscribed"));
        let mut recipients: Vec<String> = Vec::new();
        for member in lines(doc.get("members")) {
            let opted_out = unsubscribed
                .iter()
                .any(|address| address.eq_ignore_ascii_case(&member));
            let duplicate = recipients
                .iter()
                .any(|seen| seen.eq_ignore_ascii_case(&member));
            if !opted_out && !duplicate {
                recipients.push(member);
            }
        }
        Ok(recipients)
    }

    /// Opt `address` out of `group`. Returns whether it was newly added.
    ///
    /// # Errors
    ///
    /// An empty address, a group that does not exist, or a write failure.
    pub async fn unsubscribe(&self, group: &str, address: &str) -> Result<bool> {
        let address = address.trim();
        if address.is_empty() {
            return Err(ferris_core::Error::Validation(
                ferris_core::ValidationError::Required("unsubscribe address".to_string()),
            ));
        }
        let meta = ferris_schema::get_doctype(EMAIL_GROUP);
        let ctx = self.ctx(EMAIL_GROUP, meta.as_deref());
        let doc = self.engine().get(&ctx, group).await?;
        let mut unsubscribed = lines(doc.get("unsubscribed"));
        if unsubscribed
            .iter()
            .any(|listed| listed.eq_ignore_ascii_case(address))
        {
            return Ok(false);
        }
        unsubscribed.push(address.to_string());
        self.update(
            EMAIL_GROUP,
            group,
            json!({ "unsubscribed": unsubscribed.join("\n") }),
        )
        .await?;
        Ok(true)
    }
}

#[async_trait]
impl MailLog for DbMailLog {
    async fn queued(&self, job: &EmailJob) -> Result<String> {
        self.create(
            EMAIL_QUEUE,
            json!({
                "account": job.account,
                "sender": job.from,
                "recipients": recipients(job),
                "subject": job.subject,
                "message": body(job),
                "status": "Queued",
                "attempts": 0,
            }),
        )
        .await
    }

    async fn sent(
        &self,
        row: Option<&str>,
        job: &EmailJob,
        message_id: &str,
        attempt: u32,
    ) -> Result<()> {
        let name = self.ensure_row(row, job).await?;
        let communication = self.log_communication(job, &name, message_id).await?;
        let mut data = json!({
            "status": "Sent",
            "attempts": attempt,
            "sent_at": now_rfc3339(),
            "communication": communication,
        });
        if !message_id.is_empty() {
            data["message_id"] = json!(message_id);
        }
        self.update(EMAIL_QUEUE, &name, data).await
    }

    async fn failed(
        &self,
        row: Option<&str>,
        job: &EmailJob,
        error: &str,
        attempt: u32,
        retrying: bool,
    ) -> Result<()> {
        let name = self.ensure_row(row, job).await?;
        self.update(
            EMAIL_QUEUE,
            &name,
            json!({
                "status": if retrying { "Queued" } else { "Failed" },
                "attempts": attempt,
                "error": error,
            }),
        )
        .await
    }
}

/// `Email Settings.default_outgoing_account`, read through the engine.
///
/// Returns `None` when the Single cannot be read (not migrated yet, or the
/// caller has no grant): the hub then keeps its historical first-account
/// default instead of failing to boot over a preference.
pub async fn settings_default_account(pool: &Db) -> Option<String> {
    ferris_email::doctypes::register_doctypes();
    let meta = ferris_schema::get_doctype(EMAIL_SETTINGS)?;
    let mut ctx = EngineContext::new(EMAIL_SETTINGS, "");
    ctx.meta = Some(&meta);
    ctx.user = Some(service_user());
    let doc = match DocumentEngine::new(pool.clone())
        .get(&ctx, EMAIL_SETTINGS)
        .await
    {
        Ok(doc) => doc,
        Err(error) => {
            tracing::debug!(
                %error,
                "Email Settings is not readable; keeping the first configured account"
            );
            return None;
        }
    };
    doc.get("default_outgoing_account")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// The `email` queue handler: delegates the delivery and settles its row.
///
/// A failure to write the record never changes the job's outcome. Once the mail
/// left, turning a bookkeeping error into a retry would deliver it twice; once
/// it failed, the row reports the failure instead of causing it.
pub struct LoggedEmailHandler {
    inner: ferris_email::EmailJobHandler,
    log: Arc<DbMailLog>,
}

impl LoggedEmailHandler {
    #[must_use]
    pub const fn new(inner: ferris_email::EmailJobHandler, log: Arc<DbMailLog>) -> Self {
        Self { inner, log }
    }
}

#[async_trait]
impl JobHandler for LoggedEmailHandler {
    type Payload = QueuedEmailJob;
    type Output = Value;

    fn job_name(&self) -> &'static str {
        self.inner.job_name()
    }

    fn queue(&self) -> &str {
        self.inner.queue()
    }

    fn priority(&self) -> JobPriority {
        self.inner.priority()
    }

    fn max_retries(&self) -> u32 {
        self.inner.max_retries()
    }

    fn timeout(&self) -> std::time::Duration {
        self.inner.timeout()
    }

    async fn handle(&self, payload: QueuedEmailJob, ctx: JobContext) -> Result<Value> {
        let row = payload.queue_row.clone();
        let job = payload.job;
        let retrying = ctx.attempt < self.inner.max_retries();
        let outcome = self.inner.handle(job.clone(), ctx.clone()).await;
        // The SMTP path in `handlers` builds its own message, so the generated
        // `Message-ID` is not surfaced here; an empty value is honest, a minted
        // one would not match the wire.
        let recorded = match &outcome {
            Ok(_) => self.log.sent(row.as_deref(), &job, "", ctx.attempt).await,
            Err(error) => {
                self.log
                    .failed(
                        row.as_deref(),
                        &job,
                        &error.to_string(),
                        ctx.attempt,
                        retrying,
                    )
                    .await
            }
        };
        if let Err(error) = recorded {
            warn!(
                subject = %job.subject,
                %error,
                "the email left the worker but its Email Queue row could not be updated"
            );
        }
        outcome
    }
}
