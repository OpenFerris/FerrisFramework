//! The outbound calls: the refresh-token exchange and the People/Calendar
//! reads.

use std::collections::HashMap;

use ferris_core::secret::SecretValue;
use ferris_core::{AuthError, Error, Result, ValidationError, Value};

use super::config::{bool_field, str_field};
use crate::runtime::state::IntegrationsContext;

/// Exchange `refresh_token` for a fresh access token (RFC 6749 refresh
/// grant).
///
/// `secrets` is what [`settings`](super::settings) returns alongside the
/// document: the stored values are encrypted, so `settings` itself only carries
/// the mask.
///
/// # Errors
///
/// `Err` on a disabled Single, a non-2xx token endpoint, or a response
/// without an `access_token`.
pub async fn refresh_access_token(
    state: &IntegrationsContext,
    settings: &Value,
    secrets: &HashMap<String, SecretValue>,
    token_uri: &str,
) -> Result<String> {
    if !bool_field(settings, "enable", true) {
        return Err(Error::Validation(ValidationError::Invalid(
            "Google Settings is disabled".to_string(),
        )));
    }
    let client_id = str_field(settings, "client_id");
    let client_secret = secrets.get("client_secret").map_or("", SecretValue::expose);
    let refresh_token = secrets.get("refresh_token").map_or("", SecretValue::expose);
    if client_id.is_empty() || refresh_token.is_empty() {
        return Err(Error::Validation(ValidationError::Invalid(
            "Google Settings is missing client_id or refresh_token".to_string(),
        )));
    }

    let response = state
        .http
        .post(token_uri)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .map_err(|e| {
            Error::Auth(AuthError::OAuthError(format!(
                "Google token endpoint failed: {e}"
            )))
        })?;
    if !response.status().is_success() {
        return Err(Error::Auth(AuthError::OAuthError(format!(
            "Google token endpoint returned {}",
            response.status()
        ))));
    }
    let json: serde_json::Value = response.json().await.map_err(|e| {
        Error::Auth(AuthError::OAuthError(format!(
            "invalid Google token response: {e}"
        )))
    })?;
    json.get("access_token")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            Error::Validation(ValidationError::Invalid(
                "Google token response has no access_token".to_string(),
            ))
        })
}

/// One normalized contact from the People API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleContact {
    pub name: String,
    pub email: String,
    pub phone: String,
}

/// Pull the first value from a `connections[]`-style array at `path[0] =>
/// path[1]` (e.g. `names` -> `displayName`).
fn first_string(item: &serde_json::Value, path: [&str; 2]) -> String {
    item.get(path[0])
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|entry| entry.get(path[1]))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Fetch the signed-in user's People API connections (RFC 6750 bearer).
///
/// # Errors
///
/// Fails when the People API rejects the request (bad token, quota) or the
/// response is not the expected JSON shape.
pub async fn fetch_contacts(
    state: &IntegrationsContext,
    access_token: &str,
    contacts_uri: &str,
) -> Result<Vec<GoogleContact>> {
    let response = state
        .http
        .get(contacts_uri)
        .bearer_auth(access_token)
        .query(&[("personFields", "names,emailAddresses,phoneNumbers")])
        .send()
        .await
        .map_err(|e| {
            Error::Auth(AuthError::OAuthError(format!(
                "Google People API failed: {e}"
            )))
        })?;
    if !response.status().is_success() {
        return Err(Error::Auth(AuthError::OAuthError(format!(
            "Google People API returned {}",
            response.status()
        ))));
    }
    let json: serde_json::Value = response.json().await.map_err(|e| {
        Error::Auth(AuthError::OAuthError(format!(
            "invalid People API response: {e}"
        )))
    })?;
    let connections = json
        .get("connections")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut contacts = Vec::new();
    for connection in connections {
        let phone = first_string(&connection, ["phoneNumbers", "canonicalForm"]);
        let contact = GoogleContact {
            name: first_string(&connection, ["names", "displayName"]),
            email: first_string(&connection, ["emailAddresses", "value"]),
            phone: if phone.is_empty() {
                first_string(&connection, ["phoneNumbers", "value"])
            } else {
                phone
            },
        };
        if contact.name.is_empty() && contact.email.is_empty() {
            continue;
        }
        contacts.push(contact);
    }
    Ok(contacts)
}

/// One calendar from the Calendar API (id + summary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GoogleCalendar {
    pub(super) id: String,
    pub(super) summary: String,
}

/// Fetch the user's calendar list.
pub(super) async fn fetch_calendars(
    state: &IntegrationsContext,
    access_token: &str,
    calendar_uri: &str,
) -> Result<Vec<GoogleCalendar>> {
    let response = state
        .http
        .get(calendar_uri)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| {
            Error::Auth(AuthError::OAuthError(format!(
                "Google Calendar API failed: {e}"
            )))
        })?;
    if !response.status().is_success() {
        return Err(Error::Auth(AuthError::OAuthError(format!(
            "Google Calendar API returned {}",
            response.status()
        ))));
    }
    let json: serde_json::Value = response.json().await.map_err(|e| {
        Error::Auth(AuthError::OAuthError(format!(
            "invalid Calendar API response: {e}"
        )))
    })?;
    let items = json
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(items
        .into_iter()
        .filter_map(|item| {
            let id = item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if id.is_empty() {
                return None;
            }
            Some(GoogleCalendar {
                id,
                summary: item
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect())
}
