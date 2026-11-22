use crate::EmailAccountManager;
use crate::queue::EmailJob;
use crate::smtp::{Attachment, Email};
use crate::templates::TemplateEngine;
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ferris_core::{EmailError, Error, Result, ValidationError, Value};
use ferris_jobs::{JobContext, JobHandler, JobPriority};
use std::sync::Arc;

/// Job handler for sending emails
pub struct SendEmailHandler {
    accounts: Arc<EmailAccountManager>,
}

impl SendEmailHandler {
    pub fn new(accounts: Arc<EmailAccountManager>) -> Self {
        Self { accounts }
    }
}

#[async_trait]
impl JobHandler for SendEmailHandler {
    type Payload = EmailJob;
    type Output = Value;

    fn job_name(&self) -> &'static str {
        "send_email"
    }

    fn queue(&self) -> &str {
        "email"
    }

    fn priority(&self) -> JobPriority {
        JobPriority::Normal
    }

    fn max_retries(&self) -> u32 {
        3
    }

    fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(60)
    }

    async fn handle(&self, payload: EmailJob, _ctx: JobContext) -> Result<Value> {
        let account = self.accounts.get_account(&payload.account).await?;
        let client = account.create_client()?;

        let mut email = payload.into_email()?;
        // Fall back to the account's from address when the queued email did not
        // specify one.
        email.from = account.resolve_from(&email.from);

        client.send(email).await?;

        Ok(Value::from(serde_json::json!({ "status": "sent" })))
    }
}

/// Job handler for sending templated emails
pub struct SendTemplatedEmailHandler {
    accounts: Arc<EmailAccountManager>,
    templates: Arc<TemplateEngine>,
}

impl SendTemplatedEmailHandler {
    pub fn new(accounts: Arc<EmailAccountManager>, templates: Arc<TemplateEngine>) -> Self {
        Self {
            accounts,
            templates,
        }
    }
}

#[async_trait]
impl JobHandler for SendTemplatedEmailHandler {
    type Payload = EmailJob;
    type Output = Value;

    fn job_name(&self) -> &'static str {
        "send_templated_email"
    }

    fn queue(&self) -> &str {
        "email"
    }

    fn priority(&self) -> JobPriority {
        JobPriority::Normal
    }

    fn max_retries(&self) -> u32 {
        3
    }

    fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(60)
    }

    async fn handle(&self, payload: EmailJob, _ctx: JobContext) -> Result<Value> {
        let account = self.accounts.get_account(&payload.account).await?;
        let client = account.create_client()?;

        // Get template
        let template_name = payload.template.ok_or_else(|| {
            Error::Validation(ValidationError::Required("template name".to_string()))
        })?;
        let context = payload
            .template_context
            .unwrap_or(Value::Object(Default::default()));

        // Get templates from engine
        let html_template = self
            .templates
            .get_template(&format!("{}.html", template_name));
        let text_template = self
            .templates
            .get_template(&format!("{}.txt", template_name));

        let subject = payload.subject.clone();

        // Render using the engine
        let html = if let Some(tmpl) = html_template {
            Some(self.templates.render_str(&tmpl, &context, true)?)
        } else {
            None
        };

        let text = if let Some(tmpl) = text_template {
            Some(self.templates.render_str(&tmpl, &context, false)?)
        } else {
            None
        };

        // A templated job whose template has no body must fail, not send a
        // message with an empty body. `TemplateEngine::new()` registers the
        // built-ins under `<name>.html`/`<name>.txt`, so a miss means the
        // template really does not exist (or was stored without a body).
        if html.is_none() && text.is_none() {
            return Err(Error::Email(EmailError::NotFound(format!(
                "email template `{template_name}`: no `{template_name}.html` \
                 or `{template_name}.txt` registered"
            ))));
        }

        // Determine from address - use account's from_email if payload.from is empty
        let from = account.resolve_from(&payload.from);

        let mut email = Email::new(from, payload.to, subject)
            .with_cc(payload.cc)
            .with_bcc(payload.bcc);

        email.html = html;
        email.text = text;
        email.reply_to = payload.reply_to;
        email.in_reply_to = payload.in_reply_to;
        email.references = payload.references;

        for att in &payload.attachments {
            let content = STANDARD.decode(&att.content_base64).map_err(|e| {
                Error::Validation(ValidationError::Invalid(format!(
                    "Invalid attachment content: {}",
                    e
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

        client.send(email).await?;

        Ok(Value::from(serde_json::json!({ "status": "sent" })))
    }
}

/// Combined email job handler.
///
/// Both queue operations use the same [`EmailJob`] payload, so a single handler
/// can serve the whole `email` queue by routing on the job name.
pub struct EmailJobHandler {
    accounts: Arc<EmailAccountManager>,
    templates: Arc<TemplateEngine>,
}

impl EmailJobHandler {
    pub fn new(accounts: Arc<EmailAccountManager>, templates: Arc<TemplateEngine>) -> Self {
        Self {
            accounts,
            templates,
        }
    }
}

#[async_trait]
impl JobHandler for EmailJobHandler {
    type Payload = EmailJob;
    type Output = Value;

    fn job_name(&self) -> &'static str {
        "email"
    }

    fn queue(&self) -> &str {
        "email"
    }

    fn priority(&self) -> JobPriority {
        JobPriority::Normal
    }

    fn max_retries(&self) -> u32 {
        3
    }

    fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(60)
    }

    async fn handle(&self, payload: EmailJob, ctx: JobContext) -> Result<Value> {
        match ctx.job_name.as_str() {
            "send_templated_email" => {
                SendTemplatedEmailHandler::new(self.accounts.clone(), self.templates.clone())
                    .handle(payload, ctx)
                    .await
            }
            _ => {
                SendEmailHandler::new(self.accounts.clone())
                    .handle(payload, ctx)
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::EmailAccountConfig;
    use crate::smtp::SmtpConfig;

    fn account_manager() -> EmailAccountManager {
        let mut accounts = EmailAccountManager::new();
        accounts.add_account(
            crate::account::EmailAccount::new(EmailAccountConfig::new(
                "default".to_string(),
                "noreply@example.com".to_string(),
                SmtpConfig {
                    host: "localhost".to_string(),
                    port: 1025,
                    use_tls: false,
                    use_starttls: false,
                    allow_plaintext: true,
                    ..Default::default()
                },
            ))
            .expect("plaintext test account"),
        );
        accounts
    }

    fn job(template: Option<&str>) -> EmailJob {
        EmailJob {
            account: "default".to_string(),
            from: String::new(),
            to: vec!["ada@example.com".to_string()],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Hello".to_string(),
            text: None,
            html: None,
            template: template.map(str::to_string),
            template_context: None,
            reply_to: None,
            in_reply_to: None,
            references: Vec::new(),
            attachments: Vec::new(),
            priority: JobPriority::Normal,
            scheduled_at: None,
        }
    }

    fn ctx() -> JobContext {
        JobContext::new(
            "test-job".to_string(),
            "send_templated_email".to_string(),
            "email".to_string(),
        )
    }

    #[tokio::test]
    async fn templated_email_without_a_registered_body_fails() {
        let handler = SendTemplatedEmailHandler::new(
            Arc::new(account_manager()),
            Arc::new(TemplateEngine::new()),
        );
        let err = handler
            .handle(job(Some("no_such_template")), ctx())
            .await
            .expect_err("a template with no body must not send an empty email");
        assert!(
            matches!(err, Error::Email(EmailError::NotFound(_))),
            "unexpected error: {err}"
        );
    }
}
