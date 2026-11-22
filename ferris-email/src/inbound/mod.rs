//! Inbound mail: the `Inbound Email` document, its MIME parser and the store
//! that persists it.
//!
//! The IMAP client ([`crate::imap`]) is transport only. This module owns what
//! happens to a fetched message, in one place, so the poller can guarantee the
//! two rules that matter:
//!
//! * **no silent loss** — the `\Seen` flag is set only after the document is
//!   stored (see [`ImapPoller`]);
//! * **idempotence** — a message is keyed by `(account, mailbox, uid)` and by
//!   its `Message-ID`, so a re-fetch after a crash updates nothing and creates
//!   no duplicate.
//!
//! Threading is optional but real: a reply (`In-Reply-To`) whose parent is
//! already stored gets a `thread` link to that `Inbound Email`.

mod mime;
mod poller;
mod store;

pub use mime::parse;
pub use poller::{ImapPoller, PollReport, PollerConfig};
pub use store::EngineInboundStore;

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferris_core::Result as CoreResult;
use thiserror::Error;

use crate::imap::ImapError;

/// One attachment's metadata. The bytes are deliberately not kept: the
/// document row stores a description, and a file is the file service's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundAttachment {
    pub name: String,
    pub content_type: String,
    pub size: usize,
}

/// A parsed inbound message, ready to be stored.
pub struct InboundEmail {
    /// `Email Account.name` that produced it.
    pub account: String,
    /// IMAP UID inside `mailbox`.
    pub uid: u32,
    /// Mailbox name (`INBOX` by default).
    pub mailbox: String,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// Formatted `Name <address>` list, comma separated.
    pub from: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub text: String,
    pub html: String,
    /// The original RFC822 source, lossy-decoded for the document field.
    pub raw: Vec<u8>,
    pub attachments: Vec<InboundAttachment>,
    pub received_at: DateTime<Utc>,
    /// In-app notification recipient. The poller fills it from the account; a
    /// caller that does not want a notification leaves it empty.
    pub notify_user: Option<String>,
}

impl fmt::Debug for InboundEmail {
    /// Never prints the body or the raw source: inbound mail routinely carries
    /// credentials, tokens and personal data, and this struct ends up in logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InboundEmail")
            .field("account", &self.account)
            .field("mailbox", &self.mailbox)
            .field("uid", &self.uid)
            .field("message_id", &self.message_id)
            .field("from", &self.from)
            .field("subject", &self.subject)
            .field("text_bytes", &self.text.len())
            .field("html_bytes", &self.html.len())
            .field("raw_bytes", &self.raw.len())
            .field("attachments", &self.attachments.len())
            .finish_non_exhaustive()
    }
}

impl InboundEmail {
    /// Parse `raw` for `(account, mailbox, uid)`.
    ///
    /// # Errors
    ///
    /// [`InboundError::Malformed`] when the bytes are not a MIME message.
    pub fn from_raw(
        account: impl Into<String>,
        mailbox: impl Into<String>,
        uid: u32,
        raw: &[u8],
    ) -> Result<Self, InboundError> {
        mime::parse(account.into(), mailbox.into(), uid, raw)
    }

    /// The subject, or a placeholder so a notification/title is never empty.
    #[must_use]
    pub fn title(&self) -> String {
        let subject = self.subject.trim();
        if subject.is_empty() {
            "(no subject)".to_string()
        } else {
            subject.chars().take(200).collect()
        }
    }
}

/// Typed inbound failures.
#[derive(Debug, Error)]
pub enum InboundError {
    #[error("malformed MIME message: {0}")]
    Malformed(String),
    #[error("inbound mail I/O failed: {0}")]
    Imap(#[from] ImapError),
    #[error("inbound mail storage failed: {0}")]
    Store(#[from] ferris_core::Error),
}

/// Storage the processor needs. `EngineInboundStore` is the production
/// implementation; a test can implement it without a database.
#[async_trait]
pub trait InboundStore: Send + Sync {
    /// The `Inbound Email` name already stored for this mailbox/UID, if any.
    ///
    /// # Errors
    ///
    /// A database/engine failure; the poller must not mark the message seen.
    async fn existing(&self, account: &str, mailbox: &str, uid: u32) -> CoreResult<Option<String>>;

    /// The `Inbound Email` name whose `message_id` is `message_id` (threading).
    ///
    /// # Errors
    ///
    /// A database/engine failure.
    async fn find_thread(&self, message_id: &str) -> CoreResult<Option<String>>;

    /// Insert the document, with `thread` when the parent was found.
    ///
    /// # Errors
    ///
    /// A validation or database failure; the poller must not mark the message
    /// seen.
    async fn save(&self, email: &InboundEmail, thread: Option<&str>) -> CoreResult<String>;

    /// Raise the in-app notification for a stored message.
    ///
    /// The default succeeds without notifying, so a store used by a test or an
    /// embedded engine is not forced to implement the Desk feed.
    ///
    /// # Errors
    ///
    /// A database failure while storing/publishing the `Notification`.
    async fn notify(
        &self,
        _recipient: &str,
        _title: &str,
        _description: &str,
        _route: &str,
    ) -> CoreResult<()> {
        Ok(())
    }
}

/// What processing one message produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessedInbound {
    pub name: String,
    /// The message was already stored (re-fetch after a crash, or a UID reused
    /// by the server); nothing was written.
    pub duplicate: bool,
    pub notified: bool,
}

/// Parse-and-store pipeline shared by every poller.
#[derive(Clone)]
pub struct InboundProcessor<S: InboundStore> {
    store: Arc<S>,
}

impl<S: InboundStore> InboundProcessor<S> {
    #[must_use]
    pub const fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    /// Persist `email` and raise the in-app notification.
    ///
    /// A notification failure is logged and does **not** fail the call: the
    /// message is stored, and turning that into a storage failure would leave
    /// it unseen and re-deliver the same document on every poll.
    ///
    /// # Errors
    ///
    /// Any storage failure, including the duplicate lookup and the thread
    /// lookup. The caller must not flag the message seen in that case.
    pub async fn process(&self, email: &InboundEmail) -> Result<ProcessedInbound, InboundError> {
        if let Some(name) = self
            .store
            .existing(&email.account, &email.mailbox, email.uid)
            .await?
        {
            return Ok(ProcessedInbound {
                name,
                duplicate: true,
                notified: false,
            });
        }

        let thread = match email.in_reply_to.as_deref() {
            Some(parent) if !parent.is_empty() => self.store.find_thread(parent).await?,
            _ => None,
        };
        let name = self.store.save(email, thread.as_deref()).await?;

        let mut notified = false;
        if let Some(recipient) = email.notify_user.as_deref().filter(|u| !u.is_empty()) {
            let description = format!("{} — {}", email.from, email.title());
            let route = format!("/app/Inbound Email/{name}");
            match self
                .store
                .notify(recipient, &email.title(), &description, &route)
                .await
            {
                Ok(()) => notified = true,
                Err(error) => tracing::warn!(
                    account = %email.account,
                    uid = email.uid,
                    error = %error,
                    "inbound mail stored but the in-app notification failed"
                ),
            }
        }

        Ok(ProcessedInbound {
            name,
            duplicate: false,
            notified,
        })
    }
}

#[cfg(test)]
mod tests;
