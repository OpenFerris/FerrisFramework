//! The People/Calendar mirror: one upsert per fetched row, best effort.

use std::collections::HashMap;

use chrono::Utc;
use ferris_core::secret::SecretValue;
use ferris_core::{Result, Value};

use super::api::{
    GoogleCalendar, GoogleContact, fetch_calendars, fetch_contacts, refresh_access_token,
};
use super::config::{bool_field, str_field};
use crate::runtime::names::{GOOGLE_CALENDAR, GOOGLE_CONTACTS};
use crate::runtime::state::IntegrationsContext;
use crate::runtime::store;

/// Upsert a contact row keyed on `(contact_name, google_account)`.
async fn upsert_contact(
    state: &IntegrationsContext,
    contact: &GoogleContact,
    account: &str,
    synced_at: &str,
) -> Result<bool> {
    let rows = store::list(
        state,
        GOOGLE_CONTACTS,
        HashMap::from([
            ("contact_name".to_string(), Value::string(&contact.name)),
            ("google_account".to_string(), Value::string(account)),
        ]),
    )
    .await?;
    let data = Value::from_json(serde_json::json!({
        "contact_name": contact.name,
        "email": contact.email,
        "phone": contact.phone,
        "google_account": account,
        "synced_at": synced_at,
    }));
    if let Some(existing) = rows.first() {
        let name = str_field(existing, "name");
        store::update(state, GOOGLE_CONTACTS, &name, data).await?;
        Ok(false)
    } else {
        store::create(state, GOOGLE_CONTACTS, data).await?;
        Ok(true)
    }
}

/// Upsert a calendar row keyed on `calendar_id`.
async fn upsert_calendar(
    state: &IntegrationsContext,
    calendar: &GoogleCalendar,
    user: &str,
    synced_at: &str,
) -> Result<bool> {
    let rows = store::list(
        state,
        GOOGLE_CALENDAR,
        HashMap::from([("calendar_id".to_string(), Value::string(&calendar.id))]),
    )
    .await?;
    let data = Value::from_json(serde_json::json!({
        "calendar_name": calendar.summary,
        "calendar_id": calendar.id,
        "user": user,
        "sync": true,
        "last_synced_on": synced_at,
    }));
    if let Some(existing) = rows.first() {
        let name = str_field(existing, "name");
        store::update(state, GOOGLE_CALENDAR, &name, data).await?;
        Ok(false)
    } else {
        store::create(state, GOOGLE_CALENDAR, data).await?;
        Ok(true)
    }
}

/// Synchronize Google Contacts into `Google Contacts` rows. Returns the number
/// of contacts written (created or updated).
///
/// # Errors
///
/// Fails when the token refresh or People API call fails; per-contact upserts
/// are best-effort (failures are logged, never fatal).
pub async fn sync_contacts(
    state: &IntegrationsContext,
    settings: &Value,
    secrets: &HashMap<String, SecretValue>,
    token_uri: &str,
    contacts_uri: &str,
) -> Result<usize> {
    if !bool_field(settings, "enable", true) {
        return Ok(0);
    }
    let access_token = refresh_access_token(state, settings, secrets, token_uri).await?;
    let contacts = fetch_contacts(state, &access_token, contacts_uri).await?;
    let account = str_field(settings, "user");
    let synced_at = Utc::now().to_rfc3339();
    let mut written = 0;
    for contact in contacts {
        match upsert_contact(state, &contact, &account, &synced_at).await {
            Ok(_) => written += 1,
            Err(e) => tracing::warn!(error = %e, "google contact upsert failed"),
        }
    }
    Ok(written)
}

/// Synchronize Google Calendars into `Google Calendar` rows (calendar-list
/// mirror; events come with a later calendar-sync milestone). Returns how many
/// calendars were written.
///
/// # Errors
///
/// Fails when the token refresh or Calendar API call fails; per-calendar
/// upserts are best-effort.
pub async fn sync_calendars(
    state: &IntegrationsContext,
    settings: &Value,
    secrets: &HashMap<String, SecretValue>,
    token_uri: &str,
    calendar_uri: &str,
) -> Result<usize> {
    if !bool_field(settings, "enable", true) {
        return Ok(0);
    }
    let access_token = refresh_access_token(state, settings, secrets, token_uri).await?;
    let calendars = fetch_calendars(state, &access_token, calendar_uri).await?;
    let user = str_field(settings, "user");
    let synced_at = Utc::now().to_rfc3339();
    let mut written = 0;
    for calendar in calendars {
        match upsert_calendar(state, &calendar, &user, &synced_at).await {
            Ok(_) => written += 1,
            Err(e) => tracing::warn!(error = %e, "google calendar upsert failed"),
        }
    }
    Ok(written)
}
