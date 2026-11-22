//! DocTypes declared by `ferris-integrations`.
//!
//! These are the integration surface of the Desk: the OAuth/SSO providers and
//! clients, the outbound webhook and automation rules, the push/email/reminder
//! queues, and the Google and LDAP connectors. The registry itself lives in
//! `ferris-schema`; this module only declares the metas and registers them
//!.
//!
//! Two of them are **Singles** (`is_single`): `Google Settings` and
//! `LDAP Settings`. A Single is one fixed row named after its DocType, read and
//! written through `get_single`/`save_single` (§13); it has no list view and its
//! row is created lazily.
//!
//! Field sets are deliberately Frappe-shaped but trimmed to what the declaration
//! needs; a DocType grows fields when something reads them, not before.
//!
//! The declarations are grouped by the vertical the runtime implements, so a
//! connector's meta and its adapter can be read together; [`metas`] holds the
//! registration order.
#![allow(clippy::doc_markdown)]

mod automation;
mod common;
mod google;
mod ldap;
mod metas;
mod oauth;
mod push;
mod scheduling;
mod webhook;

#[cfg(test)]
mod tests;

pub use automation::{
    assignment_rule_meta, automation_flow_meta, automation_trigger_queue_meta, milestone_meta,
};
pub use common::MODULE;
pub use google::{google_calendar_meta, google_contacts_meta, google_settings_meta};
pub use ldap::ldap_settings_meta;
pub use metas::{doctype_metas, register_doctypes};
pub use oauth::{
    connected_app_meta, oauth_client_meta, oauth_provider_meta, social_login_key_meta,
};
pub use push::push_notification_meta;
pub use scheduling::{auto_email_report_meta, auto_repeat_meta, reminder_meta};
pub use webhook::webhook_meta;
