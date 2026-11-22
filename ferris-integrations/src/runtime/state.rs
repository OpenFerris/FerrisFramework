//! Process-wide state for the integrations runtime.
//!
//! [`IntegrationsContext`] bundles the one [`Db`], the outbound HTTP client, the
//! [`Mailer`] sink and the service user used for every engine operation. It is
//! cheap to clone (everything is `Arc`/handle-shaped) so hooks and jobs can each
//! hold their own copy.

use std::sync::Arc;

use ferris_auth::User;
use ferris_core::notifications;
use ferris_core::{Result, Value};
use ferris_db::Db;
use ferris_document::{DocumentEngine, EngineContext};
use serde::{Deserialize, Serialize};

use super::names::NOTIFICATION;

/// A map of field values in the shape the integrations thread between hooks,
/// jobs and the engines.
///
/// A plain `HashMap` under a name, so signatures stay free of the `BuildHasher`
/// generic `clippy::implicit_hasher` would otherwise demand (the workspace does
/// not use that generic-ization anywhere).
pub(crate) type ValueMap = std::collections::HashMap<String, Value>;

/// An email the integrations ask the host to deliver (`Auto Email Report`
/// sends these, `Reminder` with `notify = Email|Both` too).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailMessage {
    /// Explicit sender; `None` lets the host pick its default account's sender.
    pub from: Option<String>,
    /// Recipient addresses.
    pub to: Vec<String>,
    pub subject: String,
    /// Plain-text body.
    pub text: String,
}

impl EmailMessage {
    #[must_use]
    pub fn new(to: impl Into<String>, subject: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            from: None,
            to: vec![to.into()],
            subject: subject.into(),
            text: text.into(),
        }
    }
}

/// The transport the integrations hand an outgoing email to.
///
/// The crate ships [`NullMailer`] as the default sink so a missing SMTP setup
/// never fails a document save or a scheduled run; the host replaces it with a
/// real transport (e.g. ferris-email's queue) via
/// [`IntegrationsContext::with_mailer`].
pub trait Mailer: Send + Sync {
    /// Deliver `message`. Returning `Err` marks the run failed (and, for a job,
    /// lets the queue retry it).
    ///
    /// # Errors
    ///
    /// Returns `Err` when the transport could not accept the message; the caller
    /// decides whether that fails the run or is logged and retried later.
    fn send(&self, message: &EmailMessage) -> Result<()>;
}

/// Default [`Mailer`]: logs the message and succeeds.
pub struct NullMailer;

impl Mailer for NullMailer {
    fn send(&self, message: &EmailMessage) -> Result<()> {
        tracing::warn!(
            to = ?message.to,
            subject = %message.subject,
            "integrations email dropped: no mailer installed \
             (install one with IntegrationsContext::with_mailer)"
        );
        Ok(())
    }
}

fn service_user() -> User {
    let mut user = User::system_user(
        "integrations@ferris.local".to_string(),
        "Integrations".to_string(),
    );
    // `System Manager` carries the full CRUD grants every integration DocType
    // declares (`crate::doctypes`), so one principal covers all of them.
    user.roles.push("System Manager".to_string());
    user
}

/// Everything the integrations runtime needs to do its work.
#[derive(Clone)]
pub struct IntegrationsContext {
    pub db: Db,
    pub http: reqwest::Client,
    pub mailer: Arc<dyn Mailer>,
    pub service_user: User,
    /// Install-wide feature switches. The hook layer reads `webhooks`; a
    /// disabled webhook step is skipped while automation/assignment/milestone
    /// and push delivery keep running.
    pub features: ferris_core::FeatureFlags,
}

impl IntegrationsContext {
    /// Build a context with default HTTP client and the no-op mailer.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self {
            db,
            http: super::http::client(),
            mailer: Arc::new(NullMailer),
            service_user: service_user(),
            features: ferris_core::FeatureFlags::default(),
        }
    }

    /// Replace the outbound HTTP client (used by tests pointing at a local
    /// server, and by hosts with a custom TLS profile).
    #[must_use]
    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    /// Replace the email sink.
    #[must_use]
    pub fn with_mailer(mut self, mailer: Arc<dyn Mailer>) -> Self {
        self.mailer = mailer;
        self
    }

    /// Apply the operator's `[features]`. Defaults to "everything on", so the
    /// test harness and any caller that builds a context directly keep the
    /// previous behaviour.
    #[must_use]
    pub fn with_features(mut self, features: ferris_core::FeatureFlags) -> Self {
        self.features = features;
        self
    }

    /// The metadata engine, wired to this context's database.
    #[must_use]
    pub fn engine(&self) -> DocumentEngine {
        DocumentEngine::new(self.db.clone())
    }

    /// An engine context for `doctype` authored as the service user.
    ///
    /// `meta` is `None` when the DocType is not (yet) registered; the engine
    /// then skips metadata-driven validation/naming, which is the right escape
    /// hatch for a DocType a plugin registers later.
    #[must_use]
    pub fn ctx<'a>(
        &self,
        doctype: &str,
        meta: Option<&'a ferris_schema::DoctypeMeta>,
    ) -> EngineContext<'a> {
        let mut ctx = EngineContext::new(doctype, "");
        if let Some(meta) = meta {
            ctx.meta = Some(meta);
        }
        ctx.user = Some(self.service_user.clone());
        ctx
    }

    /// The context that may decrypt `Secret` fields (`read_secret`).
    ///
    /// Kept apart from [`Self::ctx`] on purpose: every plaintext reader is then
    /// one `grep read_secret`/`ctx_revealed` away, and a new call site does not
    /// gain decryption by accident. The service user already reaches the fields
    /// (System Manager); the flag is about *intent*, not a role.
    #[must_use]
    pub fn ctx_revealed<'a>(
        &self,
        doctype: &str,
        meta: Option<&'a ferris_schema::DoctypeMeta>,
    ) -> EngineContext<'a> {
        self.ctx(doctype, meta).reveal_secrets()
    }

    /// Raise an in-app `Notification` for `recipient`, storing the row and
    /// handing it to the live transport if one is installed.
    ///
    /// Mirrors `ferris-api`'s panel producer (same payload shape), so a Desk
    /// SSE subscriber renders the row identically whether an automation, a
    /// reminder, a push row or a plain login produced it.
    ///
    /// # Errors
    ///
    /// Fails when the engine rejects the row (a missing/partial `Notification`
    /// DocType registration, or a database error on insert).
    pub async fn notify(
        &self,
        recipient: &str,
        title: &str,
        description: &str,
        route: &str,
    ) -> Result<Value> {
        let meta = ferris_schema::get_doctype(NOTIFICATION);
        let ctx = self.ctx(NOTIFICATION, meta.as_deref());
        let data = Value::from_json(serde_json::json!({
            "user": recipient,
            "notification_type": "General",
            "title": title,
            "description": description,
            "route": route,
            "is_read": false,
        }));
        let created = self.engine().create_owned_by(&ctx, recipient, data).await?;
        notifications::publish(
            recipient,
            Value::from_json(serde_json::json!({
                "id": created.get("name").and_then(|v| v.as_str()).unwrap_or_default(),
                "title": title,
                "description": description,
                "route": route,
                "created_at": created.get("creation").and_then(|v| v.as_str()).unwrap_or_default(),
                "is_read": false,
            })),
        );
        Ok(created)
    }
}
