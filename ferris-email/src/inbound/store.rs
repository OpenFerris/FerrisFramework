//! The production [`InboundStore`]: documents through `ferris-document`.
//!
//! Using the engine (not a private SQL insert) is what makes inbound mail a
//! first-class record: the `Inbound Email` row goes through validation, gets a
//! version row, fires the document hooks and is subject to the `DocType`'s
//! `System Manager` permission. The in-app notification is created the same way
//! `ferris-api`'s panel creates it, then handed to the process-wide publisher
//! so an open Desk session receives it over SSE without a refetch.

use async_trait::async_trait;
use ferris_core::{Result, User, Value};
use ferris_db::Db;
use ferris_document::{DocumentEngine, EngineContext, ListQuery};
use ferris_schema::DoctypeMeta;
use serde_json::json;

use super::{InboundEmail, InboundStore};

/// `Inbound Email` and `Notification` `DocType` names.
const INBOUND_EMAIL: &str = crate::doctypes::INBOUND_EMAIL;
const NOTIFICATION: &str = "Notification";
const COMMUNICATION: &str = "Communication";
/// The account that receives an inbound notification. Its primary key is a
/// UUID, not the mailbox address, so the address has to be resolved first.
const USER: &str = "User";

/// `To` + `Cc`, comma separated: what the communication log shows as recipients.
fn recipients(to: &str, cc: &str) -> String {
    [to.trim(), cc.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Name, type and decoded size of each attachment. The bytes stay out: a file
/// belongs to the file store, and the log only has to be able to point at it.
fn attachment_summary(attachments: &[super::InboundAttachment]) -> String {
    attachments
        .iter()
        .map(|attachment| {
            format!(
                "{} ({}, {} bytes)",
                attachment.name, attachment.content_type, attachment.size
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn service_user() -> User {
    let mut user = User::system_user(
        "inbound-mail@ferris.local".to_string(),
        "Inbound Mail".to_string(),
    );
    // The DocTypes this store writes declare `System Manager`; one principal
    // covers both rows and is exactly the role the engine expects for a job.
    user.roles.push("System Manager".to_string());
    user
}

/// Store backed by the document engine.
#[derive(Clone)]
pub struct EngineInboundStore {
    db: Db,
    user: User,
}

impl EngineInboundStore {
    /// Build the store and register the crate's `DocType`s (idempotent), so a
    /// poller started before the module boot can still write its first message.
    #[must_use]
    pub fn new(db: Db) -> Self {
        crate::doctypes::register_doctypes();
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

    async fn first_matching(&self, doctype: &str, filters: ListQuery) -> Result<Option<String>> {
        let meta = ferris_schema::get_doctype(doctype);
        let ctx = self.ctx(doctype, meta.as_deref());
        let (docs, _total) = self.engine().list(&ctx, &filters).await?;
        Ok(docs
            .first()
            .and_then(|doc| doc.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// Append the `Communication` row that mirrors a stored message.
    ///
    /// The mail itself is already durable in `Inbound Email`; the log entry is
    /// derived from it, so a failure to write it must not fail the poll and
    /// re-deliver the same message (the duplicate check would then return early
    /// and the log would never be written).
    async fn log_communication(&self, email: &InboundEmail, inbound_name: &str) -> Result<String> {
        let meta = ferris_schema::get_doctype(COMMUNICATION);
        let ctx = self.ctx(COMMUNICATION, meta.as_deref());
        let content = if email.text.trim().is_empty() {
            email.html.clone()
        } else {
            email.text.clone()
        };
        let data = json!({
            "subject": email.subject.clone(),
            "sender": email.from.clone(),
            "recipients": recipients(&email.to, &email.cc),
            "content": content,
            "sent_or_received": "Received",
            "communication_type": "Email",
            "status": "Open",
            "email_account": email.account.clone(),
            "message_id": email.message_id.clone().unwrap_or_default(),
            "in_reply_to": email.in_reply_to.clone().unwrap_or_default(),
            "attachments": attachment_summary(&email.attachments),
            "reference_doctype": INBOUND_EMAIL,
            "reference_name": inbound_name,
        });
        let created = self.engine().create(&ctx, Value::from_json(data)).await?;
        created
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                ferris_core::Error::Validation(ferris_core::ValidationError::Required(
                    "Communication name".to_string(),
                ))
            })
    }

    /// The `User.name` whose `email` is `address`, if any.
    ///
    /// Read through the engine (as the store's service principal) instead of a
    /// private SQL query, so `User` follows the same table/metadata rules as
    /// every other document.
    async fn user_for_email(&self, address: &str) -> Result<Option<String>> {
        let address = address.trim();
        if address.is_empty() {
            return Ok(None);
        }
        self.first_matching(
            USER,
            ListQuery {
                filters: std::collections::HashMap::from([(
                    "email".to_string(),
                    Value::from(address),
                )]),
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
    }
}

#[async_trait]
impl InboundStore for EngineInboundStore {
    async fn existing(&self, account: &str, mailbox: &str, uid: u32) -> Result<Option<String>> {
        self.first_matching(
            INBOUND_EMAIL,
            ListQuery {
                filters: std::collections::HashMap::from([
                    ("account".to_string(), Value::from(account)),
                    ("mailbox".to_string(), Value::from(mailbox)),
                    ("imap_uid".to_string(), Value::from(i64::from(uid))),
                ]),
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
    }

    async fn find_thread(&self, message_id: &str) -> Result<Option<String>> {
        self.first_matching(
            INBOUND_EMAIL,
            ListQuery {
                filters: std::collections::HashMap::from([(
                    "message_id".to_string(),
                    Value::from(message_id),
                )]),
                limit: Some(1),
                ..Default::default()
            },
        )
        .await
    }

    async fn save(&self, email: &InboundEmail, thread: Option<&str>) -> Result<String> {
        let meta = ferris_schema::get_doctype(INBOUND_EMAIL);
        let ctx = self.ctx(INBOUND_EMAIL, meta.as_deref());
        let mut data = json!({
            "account": email.account,
            "mailbox": email.mailbox,
            "imap_uid": i64::from(email.uid),
            "message_id": email.message_id.clone().unwrap_or_default(),
            "in_reply_to": email.in_reply_to.clone().unwrap_or_default(),
            "references": email.references.join(" "),
            "from": email.from,
            "to": email.to,
            "cc": email.cc,
            "subject": email.subject,
            "body": email.text,
            "html": email.html,
            // Lossy on purpose: the column is text, and a raw source with an
            // invalid byte sequence must still be archived (the replacement
            // character is visible in the Desk, not silent corruption).
            "raw": String::from_utf8_lossy(&email.raw),
            "status": "New",
            "received_at": email.received_at.to_rfc3339(),
            "attachment_count": i64::try_from(email.attachments.len()).unwrap_or(i64::MAX),
            "attachments": email
                .attachments
                .iter()
                .map(|attachment| format!("{} ({}, {} bytes)", attachment.name, attachment.content_type, attachment.size))
                .collect::<Vec<_>>()
                .join("\n"),
        });
        // A Link field rejects an empty value, so the key is absent when there
        // is no parent rather than present as `""`.
        if let Some(parent) = thread {
            data["thread"] = serde_json::Value::String(parent.to_string());
        }
        let created = self.engine().create(&ctx, Value::from_json(data)).await?;
        let name = created
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                ferris_core::Error::Validation(ferris_core::ValidationError::Required(
                    "Inbound Email name".to_string(),
                ))
            })?;

        if let Err(error) = self.log_communication(email, &name).await {
            tracing::warn!(
                account = %email.account,
                uid = email.uid,
                %error,
                "inbound mail stored but the Communication log entry failed"
            );
        }
        Ok(name)
    }

    async fn notify(
        &self,
        recipient: &str,
        title: &str,
        description: &str,
        route: &str,
    ) -> Result<()> {
        // `recipient` is the account's address; `Notification.user` is a link to
        // `User.name`, which is a UUID. Writing the address there created a row
        // nobody owned, so the notification was never delivered (MAIL-03).
        let Some(user) = self.user_for_email(recipient).await? else {
            tracing::warn!(
                recipient = %recipient,
                "inbound mail stored but no user account has that address; \
                 the in-app notification was not created"
            );
            return Err(ferris_core::Error::Validation(
                ferris_core::ValidationError::Invalid(format!(
                    "no user account for inbound notification recipient `{recipient}`"
                )),
            ));
        };
        let meta = ferris_schema::get_doctype(NOTIFICATION);
        let ctx = self.ctx(NOTIFICATION, meta.as_deref());
        let data = Value::from_json(json!({
            "user": user.clone(),
            "notification_type": "General",
            "title": title,
            "description": description,
            "route": route,
            "is_read": false,
        }));
        // `create_owned_by`: the row belongs to the recipient, so the owner
        // rule lets them mark it read. A body `owner` would be rejected.
        let created = self.engine().create_owned_by(&ctx, &user, data).await?;
        let payload = serde_json::json!({
            "id": created.get("name").and_then(Value::as_str).unwrap_or_default(),
            "title": title,
            "description": description,
            "route": route,
            "created_at": created.get("creation").and_then(Value::as_str).unwrap_or_default(),
            "is_read": false,
        });
        ferris_core::notifications::publish(&user, Value::from_json(payload));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message with a header block, a body and one attachment.
    const RAW: &[u8] = b"From: Ann Example <ann@example.com>\r\n\
To: bob@example.com\r\n\
Cc: cy@example.com\r\n\
Subject: Quarterly report\r\n\
Message-ID: <report@example.com>\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
See the attachment.\r\n\
--b\r\n\
Content-Type: application/pdf; name=\"report.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQK\r\n\
--b--\r\n";

    fn ctx(doctype: &str) -> EngineContext<'static> {
        // The meta is leaked on purpose: the context only lives for one test and
        // the engine borrows it for the duration of the call.
        let meta: &'static DoctypeMeta = Box::leak(Box::new(
            ferris_schema::get_doctype(doctype).expect("registered"),
        ));
        let mut ctx = EngineContext::new(doctype, "");
        ctx.meta = Some(meta);
        ctx.user = Some(service_user());
        ctx
    }

    async fn rows(db: &Db, doctype: &str) -> Vec<Value> {
        DocumentEngine::new(db.clone())
            .list(
                &ctx(doctype),
                &ListQuery {
                    limit: Some(100),
                    ..Default::default()
                },
            )
            .await
            .expect("list")
            .0
    }

    /// The communication log is the Desk's outbound/inbound history: storing a
    /// message has to leave a `Received` entry that points back at it, with the
    /// envelope and the attachment metadata but not the bytes.
    #[tokio::test]
    async fn inbound_mail_is_recorded_as_a_received_communication() {
        let db = ferris_db::memory_db().await.expect("db");
        let store = EngineInboundStore::new(db.clone());
        let email = InboundEmail::from_raw("soporte", "INBOX", 9, RAW).expect("parse");

        let name = store.save(&email, None).await.expect("stored");
        assert_eq!(rows(&db, INBOUND_EMAIL).await.len(), 1);

        let communications = rows(&db, COMMUNICATION).await;
        assert_eq!(communications.len(), 1);
        let entry = &communications[0];
        let field = |key: &str| entry.get(key).and_then(Value::as_str).unwrap_or_default();

        assert_eq!(field("sent_or_received"), "Received");
        assert_eq!(field("email_account"), "soporte");
        assert_eq!(field("subject"), "Quarterly report");
        assert_eq!(field("sender"), "Ann Example <ann@example.com>");
        assert_eq!(field("recipients"), "bob@example.com, cy@example.com");
        assert_eq!(field("message_id"), "report@example.com");
        assert_eq!(field("content"), "See the attachment.");
        assert_eq!(field("reference_doctype"), INBOUND_EMAIL);
        assert_eq!(field("reference_name"), name);
        assert_eq!(
            field("attachments"),
            "report.pdf (application/pdf, 9 bytes)"
        );
    }
}
