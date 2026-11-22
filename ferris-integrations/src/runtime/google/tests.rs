//! Tests for the Google connector: the HTTP surface, the syncs and the
//! credential round-trip.

use std::collections::HashMap;

use ferris_core::Value;
use ferris_core::secret::SecretValue;

use super::config::str_field;
use super::{fetch_contacts, refresh_access_token, settings, sync_calendars, sync_contacts};
use crate::runtime::names::{GOOGLE_CALENDAR, GOOGLE_CONTACTS, GOOGLE_SETTINGS};
use crate::runtime::store;
use crate::runtime::testing::{TestServer, test_context};

fn settings_row() -> Value {
    Value::from_json(serde_json::json!({
        "enable": true,
        "client_id": "google-cid",
        "user": "ada@example.com",
    }))
}

fn secrets_row() -> HashMap<String, SecretValue> {
    HashMap::from([
        (
            "client_secret".to_string(),
            SecretValue::new("google-secret"),
        ),
        ("refresh_token".to_string(), SecretValue::new("rt-1")),
    ])
}

#[tokio::test]
async fn refresh_token_endpoint_exchanges_and_passes_bearer() {
    let server = TestServer::start_routes(vec![
        (
            "POST".into(),
            "/token".into(),
            200,
            r#"{"access_token":"tok-1","token_type":"Bearer","expires_in":3600}"#.into(),
        ),
        (
            "GET".into(),
            "/people".into(),
            200,
            r#"{"connections":[{"names":[{"displayName":"Ada Lovelace"}],"emailAddresses":[{"value":"ada@example.com"}],"phoneNumbers":[{"canonicalForm":"+1 555 0001"}]}]}"#.into(),
        ),
    ]);
    let ctx = test_context().await;
    let settings = settings_row();

    let secrets = secrets_row();
    let token = refresh_access_token(&ctx, &settings, &secrets, &format!("{}/token", server.url))
        .await
        .expect("refresh");
    assert_eq!(token, "tok-1");

    let contacts = fetch_contacts(&ctx, &token, &format!("{}/people", server.url))
        .await
        .expect("contacts");
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].name, "Ada Lovelace");
    assert_eq!(contacts[0].email, "ada@example.com");
    assert_eq!(contacts[0].phone, "+1 555 0001");

    let requests = server.received();
    let token_req = requests
        .iter()
        .find(|r| r.path.starts_with("/token"))
        .expect("token call");
    assert!(token_req.body_text().contains("grant_type=refresh_token"));
    assert!(token_req.body_text().contains("refresh_token=rt-1"));
    let people_req = requests
        .iter()
        .find(|r| r.path.starts_with("/people"))
        .expect("people call");
    assert_eq!(
        people_req.headers.get("authorization").map(String::as_str),
        Some("Bearer tok-1")
    );
}

#[tokio::test]
async fn sync_contacts_writes_and_upserts_rows() {
    let _exclusive = super::super::testing::exclusive().await;
    let server = TestServer::start_routes(vec![
        (
            "POST".into(),
            "/token".into(),
            200,
            r#"{"access_token":"tok-2","token_type":"Bearer","expires_in":3600}"#.into(),
        ),
        (
            "GET".into(),
            "/people".into(),
            200,
            r#"{"connections":[{"names":[{"displayName":"Ada"}],"emailAddresses":[{"value":"ada@example.com"}],"phoneNumbers":[]},{"names":[{"displayName":"Grace"}],"emailAddresses":[{"value":"grace@example.com"}],"phoneNumbers":[]}]}"#.into(),
        ),
    ]);
    let ctx = test_context().await;
    let settings = settings_row();

    let secrets = secrets_row();
    let written = sync_contacts(
        &ctx,
        &settings,
        &secrets,
        &format!("{}/token", server.url),
        &format!("{}/people", server.url),
    )
    .await
    .expect("sync");
    assert_eq!(written, 2);

    let rows = store::list(&ctx, GOOGLE_CONTACTS, HashMap::new())
        .await
        .expect("list");
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .any(|r| str_field(r, "email") == "ada@example.com")
    );
    assert!(
        rows.iter()
            .all(|r| str_field(r, "google_account") == "ada@example.com")
    );

    // Second run upserts, does not duplicate.
    let secrets = secrets_row();
    let written = sync_contacts(
        &ctx,
        &settings,
        &secrets,
        &format!("{}/token", server.url),
        &format!("{}/people", server.url),
    )
    .await
    .expect("sync again");
    assert_eq!(written, 2);
    let rows = store::list(&ctx, GOOGLE_CONTACTS, HashMap::new())
        .await
        .expect("list");
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
async fn sync_calendars_writes_calendar_rows() {
    let _exclusive = super::super::testing::exclusive().await;
    let server = TestServer::start_routes(vec![
        (
            "POST".into(),
            "/token".into(),
            200,
            r#"{"access_token":"tok-3","token_type":"Bearer","expires_in":3600}"#.into(),
        ),
        (
            "GET".into(),
            "/calendars".into(),
            200,
            r#"{"items":[{"id":"c-work","summary":"Work"},{"id":"c-home","summary":"Home"}]}"#
                .into(),
        ),
    ]);
    let ctx = test_context().await;
    let settings = settings_row();

    let secrets = secrets_row();
    let written = sync_calendars(
        &ctx,
        &settings,
        &secrets,
        &format!("{}/token", server.url),
        &format!("{}/calendars", server.url),
    )
    .await
    .expect("sync");
    assert_eq!(written, 2);

    let rows = store::list(&ctx, GOOGLE_CALENDAR, HashMap::new())
        .await
        .expect("list");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|r| str_field(r, "calendar_id") == "c-work"));
}

/// The H3 regression for Google: the Single stores an encrypted
/// `refresh_token`, the document read only shows the mask, and `settings()`
/// is the one call that hands the plaintext back.
#[tokio::test]
async fn settings_returns_decrypted_credentials_and_masks_the_document() {
    let _exclusive = super::super::testing::exclusive().await;
    let ctx = test_context().await;
    let meta = ferris_schema::get_doctype(GOOGLE_SETTINGS);
    let engine_ctx = ctx.ctx(GOOGLE_SETTINGS, meta.as_deref());
    ctx.engine()
        .save_single(
            &engine_ctx,
            Value::from_json(serde_json::json!({
                "enable": true,
                "client_id": "google-cid",
                "client_secret": "google-secret",
                "refresh_token": "rt-1",
                "user": "ada@example.com",
            })),
        )
        .await
        .expect("Google Settings saved");

    let (document, secrets) = settings(&ctx).await.expect("settings");
    assert_eq!(
        document.get("client_secret").and_then(Value::as_str),
        Some("********"),
        "the document keeps the mask"
    );
    assert_eq!(
        secrets.get("client_secret").map(SecretValue::expose),
        Some("google-secret")
    );
    assert_eq!(
        secrets.get("refresh_token").map(SecretValue::expose),
        Some("rt-1")
    );
}
