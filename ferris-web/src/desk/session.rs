//! Desk session plumbing (P0/P1/P2): typed access to the public auth
//! endpoints and the guarded desk endpoints (`/api/desk-layout`, `/api/search`,
//! `/api/notifications`). Used from the hydrate build (real HTTP); on SSR the
//! caller side keeps rendering the guest/login state deterministically.

use crate::api::client::{api_json, api_post_raw, api_request_raw, clear_meta_cache};
use crate::web_user::WebUser;

// Desk payload shapes, defined once in the contract so `ferris-api` and the
// browser cannot disagree about them.
pub use ferris_schema::contract::desk::{
    DeskEvent, DeskIcon, DeskIconPlacement, DeskLayout, DeskModule, DeskSearchHit,
    DeskSidebarEntry, EventFeed, Notification, NotificationFeed, NotificationSettings,
};
// The install-wide presentation settings (`/api/site-settings`); also defined
// once in the contract so a field rename is a compile error on both sides.
pub use ferris_schema::contract::SiteSettings;

/// The result of the password step of the login flow.
///
/// An account with MFA enabled answers the password check with a challenge
/// instead of a session, so the Desk has to keep that distinction rather than
/// treating the challenge body as a malformed identity.
pub enum LoginOutcome {
    /// Credentials checked out (and no second factor is owed).
    SignedIn(WebUser),
    /// The password checked out but a TOTP/backup code is still required.
    MfaRequired { challenge: String },
}

/// The OAuth providers this deployment configured, for the login page.
///
/// Empty when the OAuth surface is not mounted (the route answers 404) or when
/// no provider is configured: the login page then renders only the password
/// form. A provider is a name (`google`, `github`, …), which is also its segment
/// in `/api/oauth/{provider}/login`.
pub async fn oauth_providers() -> Result<Vec<String>, String> {
    let (status, text) = api_request_raw("GET", "/api/oauth/providers", None).await?;
    if !(200..300).contains(&status) {
        return Err(error_message(status, &text));
    }
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(value
        .get("providers")
        .and_then(serde_json::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

/// A pending TOTP enrolment: the secret, the URI an authenticator app scans
/// (or types) and the one-time backup codes.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct MfaEnrollment {
    pub secret: String,
    pub otpauth_uri: String,
    #[serde(default)]
    pub backup_codes: Vec<String>,
}

/// Start MFA enrolment: the server generates the secret and stores it disabled.
///
/// # Errors
///
/// The server's message (already enabled, no session, storage).
pub async fn mfa_enable(account_name: Option<&str>) -> Result<MfaEnrollment, String> {
    let body = serde_json::json!({ "account_name": account_name }).to_string();
    let text = api_post_raw("/api/auth/mfa/enable", &body).await?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Confirm the first TOTP code and enable MFA.
///
/// # Errors
///
/// The server's message (wrong code, no pending enrolment).
pub async fn mfa_activate(code: &str) -> Result<(), String> {
    let body = serde_json::json!({ "code": code }).to_string();
    api_post_raw("/api/auth/mfa/activate", &body)
        .await
        .map(|_| ())
}

/// Disable MFA with a TOTP or backup code.
///
/// # Errors
///
/// The server's message (wrong code, MFA not enabled).
pub async fn mfa_disable(code: &str) -> Result<(), String> {
    let body = serde_json::json!({ "code": code }).to_string();
    api_post_raw("/api/auth/mfa/disable", &body)
        .await
        .map(|_| ())
}

/// Sign in with a username/email and password.
pub async fn login(username: &str, password: &str) -> Result<LoginOutcome, String> {
    let body = serde_json::json!({ "username": username, "password": password }).to_string();
    let (status, text) = api_request_raw("POST", "/api/auth/login", Some(&body)).await?;
    let value = parse_json_body(status, &text)?;
    if value
        .get("mfa_required")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        let challenge = value
            .get("challenge")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        return Ok(LoginOutcome::MfaRequired { challenge });
    }
    let user: WebUser = serde_json::from_value(value).map_err(|e| e.to_string())?;
    clear_meta_cache();
    Ok(LoginOutcome::SignedIn(user))
}

/// Complete the second factor and return the authenticated identity.
pub async fn verify_mfa(challenge: &str, code: &str) -> Result<WebUser, String> {
    let body = serde_json::json!({ "challenge": challenge, "code": code }).to_string();
    let (status, text) = api_request_raw("POST", "/api/auth/mfa/verify", Some(&body)).await?;
    let value = parse_json_body(status, &text)?;
    let user = serde_json::from_value(value).map_err(|e| e.to_string())?;
    clear_meta_cache();
    Ok(user)
}

/// Decode a response body, turning a non-2xx into the API's human message.
fn parse_json_body(status: u16, text: &str) -> Result<serde_json::Value, String> {
    if !(200..300).contains(&status) {
        return Err(error_message(status, text));
    }
    if text.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(text).map_err(|e| e.to_string())
}

/// The `message` field of an API error body, falling back to the raw text.
fn error_message(status: u16, text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| format!("{status}: {text}"))
}

/// End the current session.
pub async fn logout() -> Result<(), String> {
    let (status, _) = api_request_raw("POST", "/api/auth/logout", Some("{}")).await?;
    if (200..300).contains(&status) {
        clear_meta_cache();
        Ok(())
    } else {
        Err(format!("logout failed ({status})"))
    }
}

/// Current identity: `Some(user)` when the session cookie is valid, `None` for
/// guests. A malformed or missing cookie is treated as "not signed in", not as
/// an error.
pub async fn me() -> Result<Option<WebUser>, String> {
    let (status, text) = api_request_raw("GET", "/api/auth/session", None).await?;
    match status {
        200 => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| e.to_string()),
        401 | 204 => Ok(None),
        _ => Err(format!("{}: {}", status, text)),
    }
}

/// Fetch the signed-in user's desk: the grid they arranged over the workspaces
/// the loaded apps declared.
///
/// Requires a session — the arrangement is the user's own — so it fails for a
/// guest rather than answering with a grid nobody asked for.
pub async fn desk_layout() -> Result<DeskLayout, String> {
    let value = api_json("GET", "/api/desk-layout", None).await?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Save the signed-in user's arrangement of the grid.
///
/// The answer is the grid the server drew from what was saved, not a bare
/// acknowledgement: a placement naming a workspace this install no longer has is
/// dropped and a workspace the arrangement never mentioned is appended, so
/// taking the answer is what keeps the desk and the stored layout in agreement.
pub async fn put_desk_layout(icons: &[DeskIconPlacement]) -> Result<DeskLayout, String> {
    let body =
        serde_json::to_string(&serde_json::json!({ "icons": icons })).map_err(|e| e.to_string())?;
    let (status, text) = api_request_raw("PUT", "/api/desk-layout", Some(&body)).await?;
    if !(200..300).contains(&status) {
        return Err(format!("{}: {}", status, text));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Run the global search endpoint.
pub async fn search(q: &str) -> Result<Vec<DeskSearchHit>, String> {
    if q.trim().is_empty() {
        return Ok(Vec::new());
    }
    let q = q.trim();
    let mut parts: Vec<String> = Vec::new();
    for b in q.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                parts.push((b as char).to_string())
            }
            _ => parts.push(format!("%{:02X}", b)),
        }
    }
    let value =
        crate::api::client::api_json("GET", &format!("/api/search?q={}", parts.join("")), None)
            .await?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Fetch notifications for the signed-in user.
pub async fn notifications() -> Result<NotificationFeed, String> {
    let value = crate::api::client::api_json("GET", "/api/notifications", None).await?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Mark every notification as read.
pub async fn mark_notifications_read() -> Result<(), String> {
    let (status, text) = api_request_raw("POST", "/api/notifications/read", Some("{}")).await?;
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(format!("{}: {}", status, text))
    }
}

/// Percent-encode one path segment so a document name cannot smuggle a slash or
/// query into the URL.
///
/// Shared with the Desk home's "Edit profile" action, which builds a form route
/// from the signed-in user's document id.
pub(crate) fn path_segment(value: &str) -> String {
    // One implementation for the whole crate graph (`ferris-web-shared`), so
    // the routes the metadata layer builds and the ones the Desk builds cannot
    // disagree about what `%` means.
    ferris_web_shared::route::path_segment(value)
}

/// Mark one notification as read.
pub async fn mark_notification_read(name: &str) -> Result<(), String> {
    let url = format!("/api/notifications/{}/read", path_segment(name));
    let (status, text) = api_request_raw("POST", &url, Some("{}")).await?;
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(format!("{}: {}", status, text))
    }
}

/// Dismiss one notification: removes it from the signed-in user's feed.
pub async fn dismiss_notification(name: &str) -> Result<(), String> {
    let url = format!("/api/notifications/{}/dismiss", path_segment(name));
    let (status, text) = api_request_raw("POST", &url, Some("{}")).await?;
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(format!("{}: {}", status, text))
    }
}

/// Fetch the upcoming events feed.
pub async fn events() -> Result<EventFeed, String> {
    let value = crate::api::client::api_json("GET", "/api/events", None).await?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Fetch the notification settings (defaults when never saved).
pub async fn notification_settings() -> Result<NotificationSettings, String> {
    let value = crate::api::client::api_json("GET", "/api/notification-settings", None).await?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Save the notification settings.
pub async fn put_notification_settings(settings: &NotificationSettings) -> Result<(), String> {
    let body = serde_json::to_string(settings).map_err(|e| e.to_string())?;
    let (status, text) = api_request_raw("PUT", "/api/notification-settings", Some(&body)).await?;
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(format!("{}: {}", status, text))
    }
}

/// Fetch the install-wide presentation settings (defaults when never saved).
pub async fn site_settings() -> Result<SiteSettings, String> {
    let value = crate::api::client::api_json("GET", "/api/site-settings", None).await?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `/api/modules` contract places group children under `"items"` (see
    /// `ferris-api::desk::workspace_json`); nested groups must deserialize.
    #[test]
    fn group_children_deserialize_from_api_items_field() {
        let json = r#"{
            "name": "operations",
            "label": "Operations",
            "module": "test",
            "icon": "LayoutGrid",
            "items": [
                { "kind": "route", "label": "Dashboard", "route": "/dashboard", "icon": "House" },
                {
                    "kind": "group",
                    "label": "Management",
                    "icon": "Folder",
                    "items": [
                        { "kind": "route", "label": "Progress report", "route": "/report/ops", "icon": "FileChartColumn" },
                        { "kind": "url", "label": "Docs", "href": "https://wiki.ferris.local", "icon": "BookOpen", "open_in_new_tab": true }
                    ]
                }
            ]
        }"#;
        let module: DeskModule = serde_json::from_str(json).expect("module parses");
        let group = module
            .items
            .iter()
            .find(|i| i.kind == "group")
            .expect("group entry present");
        let labels: Vec<_> = group.children.iter().map(|c| c.label.clone()).collect();
        assert_eq!(labels, vec!["Progress report", "Docs"]);
        assert!(group.children[1].open_in_new_tab);
    }

    /// Notification names are per-user document ids, so they must be safe as a
    /// path segment; anything else gets percent-encoded.
    #[test]
    fn path_segment_encodes_reserved_characters() {
        assert_eq!(path_segment("NOTIF-0001"), "NOTIF-0001");
        assert_eq!(path_segment("a/b c"), "a%2Fb%20c");
        assert_eq!(path_segment("x?y#z"), "x%3Fy%23z");
    }
}
