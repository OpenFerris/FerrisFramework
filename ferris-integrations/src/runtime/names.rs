//! DocType name constants shared by the runtime modules.
//!
//! The declarations in [`crate::doctypes`] spell these names out in their meta;
//! the runtime refers to them here so a rename cannot silently desynchronise
//! the two.

/// `Connected App` — an external OAuth application Ferris authenticates
/// against (client side of the OAuth dance).
pub const CONNECTED_APP: &str = "Connected App";
/// `Push Notification` — one queued in-app notification.
pub const PUSH_NOTIFICATION: &str = "Push Notification";
/// `Webhook` — an outbound HTTP call fired by a document event.
pub const WEBHOOK: &str = "Webhook";
/// `Google Contacts` — a contact synced from Google Contacts.
pub const GOOGLE_CONTACTS: &str = "Google Contacts";
/// `Google Calendar` — a calendar synced from Google Calendar.
pub const GOOGLE_CALENDAR: &str = "Google Calendar";
/// `Google Settings` — the Single holding Ferris' Google OAuth credentials.
pub const GOOGLE_SETTINGS: &str = "Google Settings";
/// `Social Login Key` — a social sign-in provider.
pub const SOCIAL_LOGIN_KEY: &str = "Social Login Key";
/// `LDAP Settings` — the Single describing the LDAP directory.
pub const LDAP_SETTINGS: &str = "LDAP Settings";
/// `OAuth Client` — an OAuth2 client registered against this install.
pub const OAUTH_CLIENT: &str = "OAuth Client";
/// `OAuth Provider` — an external OAuth2 provider Ferris delegates to.
pub const OAUTH_PROVIDER: &str = "OAuth Provider";
/// `Automation Flow` — a document-event rule and its actions.
pub const AUTOMATION_FLOW: &str = "Automation Flow";
/// `Automation Trigger Queue` — a queued automation trigger and its run state.
pub const AUTOMATION_TRIGGER_QUEUE: &str = "Automation Trigger Queue";
/// `Assignment Rule` — assigns documents of a type to a user or role.
pub const ASSIGNMENT_RULE: &str = "Assignment Rule";
/// `Milestone` — a tracked milestone on a document's lifecycle.
pub const MILESTONE: &str = "Milestone";
/// `Auto Repeat` — a recurring duplicate of a document.
pub const AUTO_REPEAT: &str = "Auto Repeat";
/// `Auto Email Report` — a report emailed on a schedule.
pub const AUTO_EMAIL_REPORT: &str = "Auto Email Report";
/// `Reminder` — a scheduled reminder about a document.
pub const REMINDER: &str = "Reminder";
/// `Notification` — the framework's in-app feed row (`ferris-api` declares the
/// meta; this crate only *writes* rows).
pub const NOTIFICATION: &str = "Notification";
/// `ToDo` — the assignment target Frappe uses. Declared by a later crate
///; when it is registered, the
/// runtime starts creating rows for it.
pub const TODO: &str = "ToDo";

/// A `Netzwerk`-independent way to spell the document events the Webhook and
/// Automation Flow DocTypes accept.
pub mod events {
    /// `after_insert`
    pub const AFTER_INSERT: &str = "after_insert";
    /// `on_update`
    pub const ON_UPDATE: &str = "on_update";
    /// `on_submit`
    pub const ON_SUBMIT: &str = "on_submit";
    /// `on_cancel`
    pub const ON_CANCEL: &str = "on_cancel";
    /// `on_trash`
    pub const ON_TRASH: &str = "on_trash";
    /// `scheduled` (automation only, reserved for a future cron runner)
    pub const SCHEDULED: &str = "scheduled";
}
