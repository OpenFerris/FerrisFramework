//! Registration order of every DocType this module declares.

use ferris_schema::DoctypeMeta;

use super::{
    assignment_rule_meta, auto_email_report_meta, auto_repeat_meta, automation_flow_meta,
    automation_trigger_queue_meta, connected_app_meta, google_calendar_meta, google_contacts_meta,
    google_settings_meta, ldap_settings_meta, milestone_meta, oauth_client_meta,
    oauth_provider_meta, push_notification_meta, reminder_meta, social_login_key_meta,
    webhook_meta,
};

/// Every DocType this crate declares, in registration order.
#[must_use]
pub fn doctype_metas() -> Vec<DoctypeMeta> {
    vec![
        connected_app_meta(),
        push_notification_meta(),
        webhook_meta(),
        google_contacts_meta(),
        google_calendar_meta(),
        google_settings_meta(),
        social_login_key_meta(),
        ldap_settings_meta(),
        oauth_client_meta(),
        oauth_provider_meta(),
        automation_flow_meta(),
        automation_trigger_queue_meta(),
        assignment_rule_meta(),
        milestone_meta(),
        auto_repeat_meta(),
        auto_email_report_meta(),
        reminder_meta(),
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
