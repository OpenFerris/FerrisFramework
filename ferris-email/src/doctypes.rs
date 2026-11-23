//! DocTypes declared by `ferris-email` (the Email section).
//!
//! The accounts, the communication log, the outbound queue, the templates and
//! the catch-all for mail Ferris could not route, plus the section's Single
//! settings row. The registry lives in
//! `ferris-schema`; this module only declares the metas and registers them
//! (§14.1).
//!
//! Field sets mirror the shapes the crate already works with
//! ([`crate::EmailAccountConfig`], [`crate::EmailTemplate`], [`crate::EmailJob`])
//! but are deliberately trimmed; a DocType grows fields when something reads
//! them.
#![allow(clippy::doc_markdown)]

use ferris_schema::{DoctypeMeta, FieldMeta, RolePermission};

/// Desk module/section every DocType here belongs to.
pub const MODULE: &str = "Email";

/// `Inbound Email` — one message fetched by the IMAP poller.
pub const INBOUND_EMAIL: &str = "Inbound Email";

/// Full CRUD for the administrator role.
#[must_use]
fn system_manager() -> RolePermission {
    RolePermission::new("System Manager")
        .read()
        .write()
        .create()
        .delete()
}

/// Read + write with neither create nor delete: the policy of a Single.
#[must_use]
fn system_manager_single() -> RolePermission {
    RolePermission::new("System Manager").read().write()
}

/// An SMTP/IMAP mailbox.
///
/// `smtp_*` configures outbound delivery; `imap_server` (with
/// `imap_username`/`imap_password`) enables the inbound poller for this
/// account. Both credentials are `Secret`, so they are encrypted at rest and
/// masked on read; the transport asks for the plaintext explicitly.
#[must_use]
pub fn email_account_meta() -> DoctypeMeta {
    DoctypeMeta::new("Email Account", MODULE)
        .description("An SMTP/IMAP mailbox: outbound delivery and inbound polling.")
        .title_field("email")
        .sort("modified", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("email")
                .label("Email")
                .required()
                .in_list_view(),
        )
        .field(
            FieldMeta::data("display_name")
                .label("Display Name")
                .in_list_view(),
        )
        .field(FieldMeta::data("from_email").label("From Email"))
        .field(FieldMeta::data("smtp_server").label("SMTP Server"))
        .field(
            FieldMeta::int("smtp_port")
                .label("SMTP Port")
                .default_value("587"),
        )
        .field(FieldMeta::secret("smtp_password").label("SMTP Password"))
        .field(
            FieldMeta::check("use_tls")
                .label("Use TLS")
                .default_value("1"),
        )
        .field(
            FieldMeta::data("imap_server")
                .label("IMAP Server")
                .description(
                    "Inbound polling is enabled for this account when this is set. \
                     Empty disables it.",
                ),
        )
        .field(
            FieldMeta::int("imap_port")
                .label("IMAP Port")
                .default_value("993"),
        )
        .field(FieldMeta::data("imap_username").label("IMAP Username"))
        .field(FieldMeta::secret("imap_password").label("IMAP Password"))
        .field(
            FieldMeta::check("default")
                .label("Default Outgoing")
                .default_value("0"),
        )
        .field(
            FieldMeta::check("enabled")
                .label("Enabled")
                .default_value("1")
                .in_list_view(),
        )
        .field(FieldMeta::text("signature").label("Signature"))
        .permission(system_manager())
}

/// One message in the communication log.
///
/// Both directions write here: the IMAP pipeline logs a `Received` row for every
/// message it stores, and the mailer logs a `Sent` row for every delivery. The
/// row keeps the envelope and the body, and describes attachments by name, type
/// and size — the bytes belong to the file store, never to this table.
#[must_use]
pub fn communication_meta() -> DoctypeMeta {
    DoctypeMeta::new("Communication", MODULE)
        .description("One message in the communication log.")
        .title_field("subject")
        .sort("creation", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(FieldMeta::data("subject").label("Subject").in_list_view())
        .field(FieldMeta::data("sender").label("Sender").in_list_view())
        .field(FieldMeta::text("recipients").label("Recipients"))
        .field(FieldMeta::text_editor("content").label("Content"))
        .field(
            FieldMeta::select("sent_or_received", "Sent\nReceived")
                .label("Sent Or Received")
                .default_value("Sent")
                .in_list_view(),
        )
        .field(FieldMeta::data("communication_type").label("Communication Type"))
        .field(
            FieldMeta::data("email_account")
                .label("Email Account")
                .description(
                    "Account the message went through, by name. Free text because an \
                     inbound mailbox may come from configuration and have no row.",
                ),
        )
        .field(
            FieldMeta::data("message_id")
                .label("Message ID")
                .description("`Message-ID` header: the received one, or the delivery's own."),
        )
        .field(FieldMeta::data("in_reply_to").label("In Reply To"))
        .field(
            FieldMeta::small_text("attachments")
                .label("Attachments")
                .description("Name, content type and decoded size of each attachment."),
        )
        .field(FieldMeta::link("reference_doctype", "DocType").label("Reference DocType"))
        .field(FieldMeta::data("reference_name").label("Reference Document"))
        .field(
            FieldMeta::select("status", "Open\nReplied\nClosed")
                .label("Status")
                .default_value("Open")
                .in_list_view(),
        )
        .field(FieldMeta::link("user", "User").label("User"))
        .permission(system_manager())
}

/// A domain Ferris sends from or accepts mail for.
#[must_use]
pub fn email_domain_meta() -> DoctypeMeta {
    DoctypeMeta::new("Email Domain", MODULE)
        .description("A domain Ferris sends from or accepts mail for.")
        .title_field("domain_name")
        .sort("domain_name", "asc")
        .allow_delete()
        .field(
            FieldMeta::data("domain_name")
                .label("Domain Name")
                .required()
                .in_list_view(),
        )
        .field(
            FieldMeta::check("enabled")
                .label("Enabled")
                .default_value("1"),
        )
        .field(FieldMeta::small_text("description").label("Description"))
        .permission(system_manager())
}

/// One queued outbound email and its delivery state.
///
/// The queue creates the row when it enqueues the job (`Queued`) and the worker
/// settles the same row: `Sent` with `sent_at` and the linked `Communication`,
/// or `Failed` with `error`. A failure that the job queue will retry stays
/// `Queued`, so the Desk shows delivery as pending rather than final.
#[must_use]
pub fn email_queue_meta() -> DoctypeMeta {
    DoctypeMeta::new("Email Queue", MODULE)
        .description("One queued outbound email and its delivery state.")
        .title_field("subject")
        .sort("creation", "asc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(FieldMeta::data("account").label("Account"))
        .field(FieldMeta::data("sender").label("Sender"))
        .field(FieldMeta::text("recipients").label("Recipients"))
        .field(FieldMeta::data("subject").label("Subject").in_list_view())
        .field(FieldMeta::text("message").label("Message"))
        .field(
            FieldMeta::select("status", "Queued\nSent\nFailed")
                .label("Status")
                .default_value("Queued")
                .in_list_view(),
        )
        .field(
            FieldMeta::int("attempts")
                .label("Attempts")
                .default_value("0")
                .description("Delivery attempts the worker made; 0 until the first one."),
        )
        .field(FieldMeta::small_text("error").label("Error"))
        .field(FieldMeta::datetime("sent_at").label("Sent At"))
        .field(FieldMeta::data("message_id").label("Message ID"))
        .field(FieldMeta::link("communication", "Communication").label("Communication"))
        .field(FieldMeta::link("reference_doctype", "DocType").label("Reference DocType"))
        .field(FieldMeta::data("reference_name").label("Reference Document"))
        .permission(system_manager())
}

/// A reusable subject/body template.
#[must_use]
pub fn email_template_meta() -> DoctypeMeta {
    DoctypeMeta::new("Email Template", MODULE)
        .description("A reusable subject/body template.")
        .title_field("template_name")
        .sort("modified", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(
            FieldMeta::data("template_name")
                .label("Template Name")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::data("subject").label("Subject").in_list_view())
        .field(
            FieldMeta::code("html_template")
                .label("HTML Template")
                .options("HTML"),
        )
        .field(FieldMeta::code("text_template").label("Text Template"))
        .field(FieldMeta::small_text("variables").label("Variables"))
        .permission(system_manager())
}

/// Single: install-wide email defaults.
#[must_use]
pub fn email_settings_meta() -> DoctypeMeta {
    DoctypeMeta::new("Email Settings", MODULE)
        .single()
        .description("Install-wide email defaults.")
        .field(
            FieldMeta::link("default_outgoing_account", "Email Account")
                .label("Default Outgoing Account"),
        )
        .field(
            FieldMeta::check("enable_outgoing")
                .label("Enable Outgoing")
                .default_value("1"),
        )
        .permission(system_manager_single())
}

/// A named group of email recipients.
///
/// Members and opt-outs are fields of the group rather than child DocTypes: the
/// group is the whole subscriber list, and one document keeps the list and its
/// unsubscribes together. A send to the group resolves `members` minus
/// `unsubscribed` and appends the opt-out instructions to each message, so a
/// mail never goes to somebody who asked not to receive it.
#[must_use]
pub fn email_group_meta() -> DoctypeMeta {
    DoctypeMeta::new("Email Group", MODULE)
        .description("A named group of email recipients.")
        .title_field("group_name")
        .sort("group_name", "asc")
        .allow_delete()
        .field(
            FieldMeta::data("group_name")
                .label("Group Name")
                .required()
                .in_list_view(),
        )
        .field(FieldMeta::small_text("description").label("Description"))
        .field(
            FieldMeta::text("members")
                .label("Members")
                .description("One recipient address per line."),
        )
        .field(
            FieldMeta::text("unsubscribed")
                .label("Unsubscribed")
                .description("Addresses that opted out of this group, one per line."),
        )
        .permission(system_manager())
}

/// One message the IMAP poller fetched from an account's mailbox.
///
/// The row is the durable record: the raw source is kept for re-parsing and
/// audit, the decoded bodies for the Desk, and `status` tracks whether the
/// message was filed, linked to a document or archived. `uid`/`mailbox` make a
/// re-fetch idempotent; `message_id` and `in_reply_to` carry the thread.
#[must_use]
pub fn inbound_email_meta() -> DoctypeMeta {
    DoctypeMeta::new(INBOUND_EMAIL, MODULE)
        .description("One inbound message fetched by the IMAP poller.")
        .title_field("subject")
        .sort("received_at", "desc")
        .allow_delete()
        .field(FieldMeta::section_break("section_details").label("Details"))
        .field(FieldMeta::data("subject").label("Subject").in_list_view())
        .field(FieldMeta::data("from").label("From").in_list_view())
        .field(FieldMeta::text("to").label("To"))
        .field(FieldMeta::text("cc").label("Cc"))
        .field(FieldMeta::text("body").label("Body"))
        .field(FieldMeta::text_editor("html").label("HTML Body"))
        .field(FieldMeta::code("raw").label("Raw Message").options("Text"))
        .field(
            FieldMeta::select("status", "New\nReplied\nArchived\nSpam")
                .label("Status")
                .default_value("New")
                .in_list_view(),
        )
        .field(FieldMeta::data("message_id").label("Message ID"))
        .field(FieldMeta::data("in_reply_to").label("In Reply To"))
        .field(FieldMeta::small_text("references").label("References"))
        .field(
            FieldMeta::link("thread", INBOUND_EMAIL)
                .label("Thread")
                .description("The earlier Inbound Email this one replies to, when known."),
        )
        .field(
            FieldMeta::data("account")
                .label("Account")
                .description("Email Account.name that fetched it."),
        )
        .field(FieldMeta::data("mailbox").label("Mailbox"))
        .field(FieldMeta::int("imap_uid").label("IMAP UID"))
        .field(FieldMeta::datetime("received_at").label("Received At"))
        .field(
            FieldMeta::int("attachment_count")
                .label("Attachments")
                .default_value("0"),
        )
        .field(
            FieldMeta::small_text("attachments")
                .label("Attachment List")
                .description("Name, content type and decoded size of each attachment."),
        )
        .permission(system_manager())
}

/// Every DocType this crate declares, in registration order.
#[must_use]
pub fn doctype_metas() -> Vec<DoctypeMeta> {
    vec![
        email_account_meta(),
        inbound_email_meta(),
        communication_meta(),
        email_domain_meta(),
        email_queue_meta(),
        email_template_meta(),
        email_settings_meta(),
        email_group_meta(),
    ]
}

/// Register this crate's DocTypes in the global schema registry.
///
/// Idempotent: re-registering overwrites the same keys, so it is safe on every
/// boot and after a hot reload. Tables themselves are created lazily by the
/// document engine the first time a DocType is touched.
pub fn register_doctypes() {
    for meta in doctype_metas() {
        ferris_schema::register_doctype(meta);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_schema::get_doctype;

    #[test]
    fn every_doctype_registers_with_fields() {
        register_doctypes();
        for meta in doctype_metas() {
            let got = get_doctype(&meta.name).expect("registered");
            assert_eq!(got.module, MODULE, "{}", meta.name);
            assert!(!got.fields.is_empty(), "{} needs fields", meta.name);
        }
    }

    #[test]
    fn only_email_settings_is_a_single() {
        let singles: Vec<_> = doctype_metas()
            .into_iter()
            .filter(|m| m.is_single)
            .map(|m| m.name)
            .collect();
        assert_eq!(singles, vec!["Email Settings"]);
    }

    #[test]
    fn register_is_idempotent() {
        register_doctypes();
        register_doctypes();
        assert!(get_doctype("Email Account").is_some());
        assert!(get_doctype("Email Settings").is_some());
    }

    /// The two mailbox credentials are **recoverable** secrets: SMTP/IMAP need
    /// the plaintext to authenticate. Declaring them `Password` made them hash
    /// on write and read back as a mask (H3), so no transport could ever use
    /// them.
    #[test]
    fn the_mailbox_credentials_are_recoverable_secrets() {
        register_doctypes();
        let meta = get_doctype("Email Account").expect("registered");
        for fieldname in ["smtp_password", "imap_password"] {
            let field = meta
                .fields
                .iter()
                .find(|field| field.fieldname == fieldname)
                .expect("declared");
            assert_eq!(field.fieldtype, "Secret", "{fieldname}");
            assert!(!field.in_list_view, "{fieldname} must not be a list column");
            assert!(!field.unique, "{fieldname} must not be unique");
            assert!(!field.search_index, "{fieldname} must not be indexed");
        }
    }
}
