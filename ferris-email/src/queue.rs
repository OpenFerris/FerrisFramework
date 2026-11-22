//! Email queue for async sending

use crate::account::EmailAccount;
use crate::smtp::{Attachment, Email};
use chrono::{DateTime, Utc};
use ferris_core::{EmailError, Error, Result, ValidationError, Value};
use ferris_jobs::{JobPriority, JobQueue};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Email job for queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailJob {
    pub account: String,
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub template: Option<String>,
    pub template_context: Option<Value>,
    pub reply_to: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub attachments: Vec<AttachmentRef>,
    pub priority: JobPriority,
    pub scheduled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRef {
    /// Identifier in the file store, when the attachment was uploaded there.
    pub file_id: String,
    pub filename: Option<String>,
    pub content_type: String,
    /// Base64-encoded bytes, kept so queueing an email never drops the
    /// attachment when no file-store reference is available.
    pub content_base64: String,
    pub inline: bool,
    pub content_id: Option<String>,
}

/// The durable record of outbound mail: the `Email Queue` row and, once sent,
/// the `Communication` it points at.
///
/// The queue knows *what* to send but not how to store a document: those rows go
/// through the document engine and the host's database, which live in
/// `ferris-server`. The host installs an implementation with
/// [`EmailQueue::with_log`]; without one the queue still delivers but leaves no
/// row, so the server always installs it.
#[async_trait::async_trait]
pub trait MailLog: Send + Sync {
    /// Create the `Queued` row for `job` and return the row's name.
    async fn queued(&self, job: &EmailJob) -> Result<String>;

    /// Mark `row` (created by the queue, or by this call when `None`) as sent,
    /// with the `Message-ID` the message went out with.
    async fn sent(
        &self,
        row: Option<&str>,
        job: &EmailJob,
        message_id: &str,
        attempt: u32,
    ) -> Result<()>;

    /// Record a delivery failure. `retrying` keeps the row `Queued` because the
    /// job queue will try again; the last attempt marks it `Failed`.
    async fn failed(
        &self,
        row: Option<&str>,
        job: &EmailJob,
        error: &str,
        attempt: u32,
        retrying: bool,
    ) -> Result<()>;
}

/// The payload the worker deserializes: the historical [`EmailJob`] plus the
/// `Email Queue` row the queue already created for it.
///
/// The row name rides as an extra payload key rather than an [`EmailJob`] field
/// because that struct's shape is fixed by its consumers. A job queued without
/// the key (any other producer) settles by creating its row at that point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedEmailJob {
    #[serde(flatten)]
    pub job: EmailJob,
    #[serde(default)]
    pub queue_row: Option<String>,
}

impl EmailJob {
    /// Rebuild an [`Email`] from a queued job, decoding any inline attachments.
    ///
    /// Attachments referenced only by `file_id` cannot be resolved here (that
    /// needs the file store); the caller should fetch those separately.
    pub fn into_email(&self) -> Result<Email> {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let mut email = Email::new(self.from.clone(), self.to.clone(), self.subject.clone())
            .with_cc(self.cc.clone())
            .with_bcc(self.bcc.clone());
        email.text = self.text.clone();
        email.html = self.html.clone();
        email.reply_to = self.reply_to.clone();
        email.in_reply_to = self.in_reply_to.clone();
        email.references = self.references.clone();

        for att in &self.attachments {
            let content = STANDARD.decode(&att.content_base64).map_err(|e| {
                Error::Validation(ValidationError::Invalid(format!(
                    "Invalid attachment content: {e}"
                )))
            })?;
            let mut attachment = Attachment::new(
                att.filename
                    .clone()
                    .unwrap_or_else(|| "attachment".to_string()),
                att.content_type.clone(),
                content,
            );
            if att.inline {
                attachment.inline = true;
                attachment.content_id = att.content_id.clone();
            }
            email.attachments.push(attachment);
        }
        Ok(email)
    }
}

/// Email queue
pub struct EmailQueue {
    job_queue: Arc<JobQueue>,
    accounts: Arc<EmailAccountManager>,
    default_account: String,
    log: Option<Arc<dyn MailLog>>,
}

impl EmailQueue {
    pub fn new(
        job_queue: Arc<JobQueue>,
        accounts: Arc<EmailAccountManager>,
        default_account: String,
    ) -> Self {
        Self {
            job_queue,
            accounts,
            default_account,
            log: None,
        }
    }

    /// Install the delivery log, so every queued, sent and failed message
    /// leaves an `Email Queue` row (see [`MailLog`]).
    #[must_use]
    pub fn with_log(mut self, log: Arc<dyn MailLog>) -> Self {
        self.log = Some(log);
        self
    }

    /// Send email immediately (bypass queue)
    pub async fn send_now(&self, email: Email) -> Result<()> {
        let account = self.accounts.get_account(&self.default_account).await?;
        let job = Self::job_for(email, &self.default_account, &account);
        let row = self.log_queued(&job).await?;
        let client = account.create_client()?;
        match client.send_tracked(job.clone().into_email()?).await {
            Ok(message_id) => {
                if let Some(log) = &self.log
                    && let Err(error) = log.sent(row.as_deref(), &job, &message_id, 1).await
                {
                    tracing::error!(%error, "sent mail was not recorded in its Email Queue row");
                }
                Ok(())
            }
            Err(error) => {
                self.record_failure(row.as_deref(), &job, &error.to_string(), 1, false)
                    .await;
                Err(error)
            }
        }
    }

    /// Queue email for async sending
    pub async fn queue_email(&self, email: Email) -> Result<String> {
        self.queue_email_with_account(email, &self.default_account)
            .await
    }

    /// Queue email with specific account
    pub async fn queue_email_with_account(
        &self,
        email: Email,
        account_name: &str,
    ) -> Result<String> {
        let account = self.accounts.get_account(account_name).await?;
        let job = Self::job_for(email, account_name, &account);
        self.enqueue_job(job, "send_email", |entry| {
            entry.max_retries = 3;
            entry.timeout = std::time::Duration::from_secs(60);
        })
        .await
    }

    /// Queue templated email
    pub async fn queue_templated_email(
        &self,
        account: &str,
        template: &str,
        context: Value,
        to: Vec<String>,
        subject: String,
    ) -> Result<String> {
        // Get the account to use its from_email as default
        let account_obj = self.accounts.get_account(account).await?;
        let default_from = account_obj
            .config
            .from_email
            .clone()
            .unwrap_or_else(|| "noreply@ferris.local".to_string());

        let job = EmailJob {
            account: account.to_string(),
            from: default_from,
            to,
            cc: vec![],
            bcc: vec![],
            subject,
            text: None,
            html: None,
            template: Some(template.to_string()),
            template_context: Some(context),
            reply_to: None,
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
            priority: JobPriority::Normal,
            scheduled_at: None,
        };

        self.enqueue_job(job, "send_templated_email", |_| {}).await
    }

    /// Schedule email for later
    pub async fn schedule_email(&self, email: Email, send_at: DateTime<Utc>) -> Result<String> {
        let account = self.accounts.get_account(&self.default_account).await?;
        let mut job = Self::job_for(email, &self.default_account, &account);
        job.scheduled_at = Some(send_at);
        self.enqueue_job(job, "send_email", |entry| {
            entry.scheduled_at = Some(send_at);
        })
        .await
    }

    /// Describe `email` as the job that will deliver it.
    fn job_for(email: Email, account_name: &str, account: &EmailAccount) -> EmailJob {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        EmailJob {
            account: account_name.to_string(),
            from: account.resolve_from(&email.from),
            to: email.to,
            cc: email.cc,
            bcc: email.bcc,
            subject: email.subject,
            text: email.text,
            html: email.html,
            template: None,
            template_context: None,
            reply_to: email.reply_to,
            in_reply_to: email.in_reply_to,
            references: email.references,
            attachments: email
                .attachments
                .iter()
                .map(|a| AttachmentRef {
                    file_id: String::new(),
                    filename: Some(a.filename.clone()),
                    content_type: a.content_type.clone(),
                    content_base64: STANDARD.encode(&a.content),
                    inline: a.inline,
                    content_id: a.content_id.clone(),
                })
                .collect(),
            priority: JobPriority::Normal,
            scheduled_at: None,
        }
    }

    /// Create `job`'s delivery row and enqueue it.
    ///
    /// `adjust` runs on the job entry first, so each entry point keeps its own
    /// retry/timeout/schedule policy without duplicating the row bookkeeping.
    async fn enqueue_job(
        &self,
        job: EmailJob,
        name: &str,
        adjust: impl FnOnce(&mut ferris_jobs::Job),
    ) -> Result<String> {
        let row = self.log_queued(&job).await?;
        let mut payload = serde_json::to_value(&job)?;
        if let Some(row) = row.as_deref() {
            payload["queue_row"] = serde_json::Value::String(row.to_string());
        }

        let mut entry = ferris_jobs::Job::new(name.to_string(), ferris_core::Value::from(payload));
        entry.queue = "email".to_string();
        entry.priority = ferris_jobs::JobPriority::Normal;
        adjust(&mut entry);

        match self.job_queue.enqueue(entry).await {
            Ok(id) => Ok(id),
            Err(error) => {
                // The row exists but nothing will ever drain it: record why,
                // instead of leaving a `Queued` message that is not queued.
                self.record_failure(row.as_deref(), &job, &error.to_string(), 0, false)
                    .await;
                Err(error)
            }
        }
    }

    async fn log_queued(&self, job: &EmailJob) -> Result<Option<String>> {
        match &self.log {
            Some(log) => log.queued(job).await.map(Some),
            None => Ok(None),
        }
    }

    async fn record_failure(
        &self,
        row: Option<&str>,
        job: &EmailJob,
        error: &str,
        attempt: u32,
        retrying: bool,
    ) {
        let Some(log) = &self.log else { return };
        if let Err(log_error) = log.failed(row, job, error, attempt, retrying).await {
            tracing::error!(%log_error, "could not record the delivery failure");
        }
    }

    /// Get queue stats
    pub async fn get_stats(&self) -> Result<EmailQueueStats> {
        // The email queue instance is typically backed by a JobQueue configured
        // for a different (e.g. global/default) queue, so query the *email*
        // queue specifically.
        let stats = self.job_queue.get_queue_stats("email").await?;

        Ok(EmailQueueStats {
            queue_name: "email".to_string(),
            queued: stats.queued,
            processing: stats.processing,
            completed: stats.completed,
            failed: stats.failed,
            dead_letter: stats.dead_letter,
        })
    }
}

/// Email queue statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmailQueueStats {
    pub queue_name: String,
    pub queued: usize,
    pub processing: usize,
    pub completed: usize,
    pub failed: usize,
    pub dead_letter: usize,
}

/// Email account manager.
///
/// Accounts come from configuration and live only as long as the process; SMTP
/// credentials are not written to the database by anything here.
pub struct EmailAccountManager {
    accounts: HashMap<String, EmailAccount>,
}

use std::collections::HashMap;

impl EmailAccountManager {
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
        }
    }

    /// Add email account
    pub fn add_account(&mut self, account: EmailAccount) {
        self.accounts.insert(account.config.name.clone(), account);
    }

    /// Get account by name
    pub async fn get_account(&self, name: &str) -> Result<EmailAccount> {
        self.accounts.get(name).cloned().ok_or_else(|| {
            Error::Email(EmailError::NotFound(format!(
                "Email account {} not found",
                name
            )))
        })
    }

    /// Get default account
    pub async fn get_default_account(&self) -> Result<EmailAccount> {
        if let Some(account) = self.accounts.get("default") {
            return Ok(account.clone());
        }

        // Return first account
        self.accounts.values().next().cloned().ok_or_else(|| {
            Error::Email(EmailError::NotFound(
                "No email accounts configured".to_string(),
            ))
        })
    }

    /// List all accounts
    pub fn list_accounts(&self) -> Vec<EmailAccount> {
        self.accounts.values().cloned().collect()
    }

    /// Remove account
    pub fn remove_account(&mut self, name: &str) -> bool {
        self.accounts.remove(name).is_some()
    }
}

impl Default for EmailAccountManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_email_job() {
        let job = EmailJob {
            account: "default".to_string(),
            from: "sender@example.com".to_string(),
            to: vec!["test@example.com".to_string()],
            cc: vec![],
            bcc: vec![],
            subject: "Test".to_string(),
            text: Some("Hello".to_string()),
            html: None,
            template: None,
            template_context: None,
            reply_to: None,
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
            priority: JobPriority::Normal,
            scheduled_at: None,
        };

        let json = serde_json::to_string(&job).unwrap();
        let parsed: EmailJob = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.account, "default");
    }

    #[test]
    fn test_attachment_survives_queue_roundtrip() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let email = Email::new(
            "sender@example.com".to_string(),
            vec!["test@example.com".to_string()],
            "With attachment".to_string(),
        )
        .with_attachment(Attachment::new(
            "hello.txt".to_string(),
            "text/plain".to_string(),
            b"hello world".to_vec(),
        ));

        // Mimic the queue's attachment mapping.
        let attachments: Vec<AttachmentRef> = email
            .attachments
            .iter()
            .map(|a| AttachmentRef {
                file_id: String::new(),
                filename: Some(a.filename.clone()),
                content_type: a.content_type.clone(),
                content_base64: STANDARD.encode(&a.content),
                inline: a.inline,
                content_id: a.content_id.clone(),
            })
            .collect();

        let job = EmailJob {
            account: "default".to_string(),
            from: "sender@example.com".to_string(),
            to: vec!["test@example.com".to_string()],
            cc: vec![],
            bcc: vec![],
            subject: "With attachment".to_string(),
            text: None,
            html: None,
            template: None,
            template_context: None,
            reply_to: None,
            in_reply_to: None,
            references: vec![],
            attachments,
            priority: JobPriority::Normal,
            scheduled_at: None,
        };

        let json = serde_json::to_string(&job).unwrap();
        let parsed: EmailJob = serde_json::from_str(&json).unwrap();
        let rebuilt = parsed.into_email().unwrap();

        assert_eq!(rebuilt.attachments.len(), 1);
        assert_eq!(rebuilt.attachments[0].filename, "hello.txt");
        assert_eq!(rebuilt.attachments[0].content, b"hello world");
    }

    /// The worker deserializes the widened payload: the row name has to survive
    /// next to the historical `EmailJob` keys, or the worker creates a second
    /// row for a message the queue already recorded.
    #[test]
    fn the_worker_payload_keeps_the_queue_row_next_to_the_job() {
        let job = EmailJob {
            account: "default".to_string(),
            from: "sender@example.com".to_string(),
            to: vec!["test@example.com".to_string()],
            cc: vec![],
            bcc: vec![],
            subject: "Queued".to_string(),
            text: Some("Hello".to_string()),
            html: None,
            template: None,
            template_context: None,
            reply_to: None,
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
            priority: JobPriority::Normal,
            scheduled_at: None,
        };
        let mut payload = serde_json::to_value(&job).expect("payload");
        payload["queue_row"] = serde_json::json!("EQ-1");

        let queued: QueuedEmailJob = serde_json::from_value(payload).expect("deserialize");
        assert_eq!(queued.queue_row.as_deref(), Some("EQ-1"));
        assert_eq!(queued.job.subject, "Queued");
        assert_eq!(queued.job.to, vec!["test@example.com".to_string()]);

        // A producer that never logged the row still deserializes.
        let bare: QueuedEmailJob =
            serde_json::from_value(serde_json::to_value(&job).expect("payload"))
                .expect("deserialize");
        assert!(bare.queue_row.is_none());
    }
}
