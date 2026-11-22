//! OAuth adapters over `ferris-auth`'s real OAuth2/OIDC client.
//!
//! The catalog models three DocTypes that all describe an OAuth client —
//! [`Social Login Key`](super::names::SOCIAL_LOGIN_KEY) (this install as the
//! client of an external provider), [`OAuth Provider`](super::names::OAUTH_PROVIDER)
//! (same shape, self-documenting) and [`Connected App`](super::names::CONNECTED_APP)
//! — and this module turns those stored rows into the
//! [`OAuthProviderConfig`] [`ferris_auth::oauth`] understands, then drives both
//! halves of the dance:
//!
//! * [`start`] builds the provider's authorization URL (state + PKCE S256
//!   challenge) to bounce the browser to.
//! * [`exchange_code`] swaps the code the provider sends back for tokens at the
//!   configured token endpoint; [`AuthorizeState::complete`] first checks the
//!   `state` the provider echoed.
//! * [`load_social_login_keys`] / [`load_connected_apps`] turn the enabled
//!   DocType rows into provider configs, so the runtime reads OAuth clients
//!   from the database instead of a static list.
//!
//! The token call is made by `ferris-auth`'s client, which is exercised against
//! a local test endpoint in this module's tests — the same transport the
//! running server uses.

use ferris_auth::oauth::{OAuthClient, OAuthProviderConfig, PkceMethod, TokenResponse};
use ferris_core::secret::SecretValue;
use ferris_core::{Error, Result, ValidationError, Value};
use serde::{Deserialize, Serialize};

use super::names::{CONNECTED_APP, SOCIAL_LOGIN_KEY};
use super::state::{IntegrationsContext, ValueMap};
use super::store;

fn str_field(doc: &Value, key: &str) -> String {
    doc.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// What [`start`] hands back: navigate the browser to `url`, persist `state`
/// (and `pkce_verifier`) until the callback.
///
/// The value is `Serialize` so a host can store it server-side (keyed by the
/// browser session) between the redirect and the callback; never trust a
/// `state`/verifier supplied by the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizeState {
    pub url: String,
    pub state: String,
    pub pkce_verifier: Option<String>,
}

impl AuthorizeState {
    /// Whether the provider echoed the `state` this flow minted (RFC 6749
    /// §10.12), compared without an early exit.
    #[must_use]
    pub fn accepts_state(&self, echoed: &str) -> bool {
        constant_time_eq(self.state.as_bytes(), echoed.as_bytes())
    }

    /// Validate the echoed `state` and exchange the code for tokens.
    ///
    /// # Errors
    ///
    /// Rejects a mismatched `state` before any network call; otherwise the
    /// token endpoint's error propagates from [`exchange_code`].
    pub async fn complete(
        self,
        config: OAuthProviderConfig,
        code: String,
        echoed_state: &str,
    ) -> Result<TokenResponse> {
        if !self.accepts_state(echoed_state) {
            return Err(Error::Validation(ValidationError::Invalid(
                "OAuth state does not match the flow that minted it".to_string(),
            )));
        }
        exchange_code(config, code, self.pkce_verifier).await
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&a, &b) in left.iter().zip(right) {
        diff |= a ^ b;
    }
    diff == 0
}

/// A `Check` field as a boolean (`true`/`"1"`/`1`), accepting the shapes the
/// document engine stores.
fn flag(doc: &Value, key: &str) -> Option<bool> {
    match doc.get(key) {
        Some(Value::Bool(value)) => Some(*value),
        Some(Value::Number(value)) => value.as_i64().map(|number| number != 0),
        Some(Value::String(value)) => match value.as_str() {
            "1" | "true" | "yes" => Some(true),
            "0" | "false" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Whether a configuration row is enabled (absent → enabled).
fn is_enabled(doc: &Value) -> bool {
    flag(doc, "enabled").unwrap_or(true)
}

fn build_config(
    name: &str,
    doc: &Value,
    secret: Option<&SecretValue>,
    require_redirect: bool,
) -> Result<OAuthProviderConfig> {
    let client_id = str_field(doc, "client_id");
    let client_secret = secret.map_or_else(String::new, |value| value.expose().to_string());
    let auth_url = str_field(doc, "authorization_uri");
    let token_url = str_field(doc, "token_uri");
    let userinfo_uri = str_field(doc, "userinfo_uri");
    let redirect_uri = str_field(doc, "redirect_uri");

    if name.is_empty() {
        return Err(Error::Validation(ValidationError::Required(
            "OAuth provider name".to_string(),
        )));
    }
    if client_id.is_empty() || auth_url.is_empty() || token_url.is_empty() {
        return Err(Error::Validation(ValidationError::Required(format!(
            "OAuth provider `{name}` client_id, authorization_uri or token_uri"
        ))));
    }
    if require_redirect && redirect_uri.is_empty() {
        return Err(Error::Validation(ValidationError::Required(format!(
            "OAuth provider `{name}` redirect_uri"
        ))));
    }

    Ok(OAuthProviderConfig {
        name: name.to_string(),
        client_id,
        client_secret,
        auth_url,
        token_url,
        user_info_url: (!userinfo_uri.is_empty()).then_some(userinfo_uri),
        device_auth_url: None,
        scopes: split_scopes(&str_field(doc, "scopes")),
        // PKCE is on by default (RFC 7636 S256); a `pkce` custom field set to
        // `0`/`false` opts a provider that rejects it out.
        pkce_enabled: flag(doc, "pkce").unwrap_or(true),
        pkce_method: PkceMethod::S256,
        redirect_uris: if redirect_uri.is_empty() {
            Vec::new()
        } else {
            vec![redirect_uri]
        },
    })
}

fn split_scopes(raw: &str) -> Vec<String> {
    raw.split(|c: char| " ,\n".contains(c))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// A `Social Login Key` or `OAuth Provider` row as a provider config.
///
/// `secret` is the decrypted `client_secret` (the row's own value is the read
/// mask). Prefer [`load_provider`], which reads both halves together.
///
/// # Errors
///
/// Fails when the row is missing the provider name, `client_id`,
/// `authorization_uri` or `token_uri`, or the required `redirect_uri`.
pub fn provider_config(doc: &Value, secret: Option<&SecretValue>) -> Result<OAuthProviderConfig> {
    build_config(&str_field(doc, "provider_name"), doc, secret, true)
}

/// A `Connected App` row as a provider config (name comes from `app_name`).
///
/// # Errors
///
/// Same validation contract as [`provider_config`].
pub fn connected_app_config(
    doc: &Value,
    secret: Option<&SecretValue>,
) -> Result<OAuthProviderConfig> {
    build_config(&str_field(doc, "app_name"), doc, secret, true)
}

/// Load a `Social Login Key`/`OAuth Provider` document and its decrypted
/// `client_secret` in one call — the safe default for a caller that only has a
/// document name.
///
/// # Errors
///
/// Propagates the read, decryption and configuration errors; see
/// [`provider_config`].
pub async fn load_provider(
    state: &IntegrationsContext,
    doctype: &str,
    name: &str,
) -> Result<OAuthProviderConfig> {
    let document = store::get(state, doctype, name).await?;
    let mut secrets = store::secrets(state, doctype, name, &["client_secret"]).await?;
    let secret = secrets.remove("client_secret");
    provider_config(&document, secret.as_ref())
}

/// [`load_provider`] for the `Connected App` DocType.
///
/// # Errors
///
/// See [`load_provider`].
pub async fn load_connected_app(
    state: &IntegrationsContext,
    name: &str,
) -> Result<OAuthProviderConfig> {
    let document = store::get(state, CONNECTED_APP, name).await?;
    let mut secrets = store::secrets(state, CONNECTED_APP, name, &["client_secret"]).await?;
    let secret = secrets.remove("client_secret");
    connected_app_config(&document, secret.as_ref())
}

/// Every enabled `Social Login Key` row as a provider config.
///
/// Disabled rows are skipped; an enabled row with an invalid configuration
/// fails the whole call instead of disappearing, so a typo is visible to the
/// operator rather than silently removing a login option.
///
/// # Errors
///
/// Propagates the list/secret read errors and the validation from
/// [`provider_config`].
pub async fn load_social_login_keys(
    state: &IntegrationsContext,
) -> Result<Vec<OAuthProviderConfig>> {
    load_all(state, SOCIAL_LOGIN_KEY, provider_config).await
}

/// Every enabled `Connected App` row as a provider config.
///
/// # Errors
///
/// Same contract as [`load_social_login_keys`], with [`connected_app_config`].
pub async fn load_connected_apps(state: &IntegrationsContext) -> Result<Vec<OAuthProviderConfig>> {
    load_all(state, CONNECTED_APP, connected_app_config).await
}

async fn load_all(
    state: &IntegrationsContext,
    doctype: &str,
    build: fn(&Value, Option<&SecretValue>) -> Result<OAuthProviderConfig>,
) -> Result<Vec<OAuthProviderConfig>> {
    let rows = store::list(state, doctype, ValueMap::new()).await?;
    let mut configs = Vec::with_capacity(rows.len());
    for row in rows {
        if !is_enabled(&row) {
            continue;
        }
        let Some(name) = row.get("name").and_then(|value| value.as_str()) else {
            continue;
        };
        let mut secrets = store::secrets(state, doctype, name, &["client_secret"]).await?;
        let secret = secrets.remove("client_secret");
        configs.push(build(&row, secret.as_ref())?);
    }
    Ok(configs)
}

/// Build the authorization URL to navigate the browser to.
///
/// # Errors
///
/// Fails when the provider configuration is rejected by the OAuth client.
pub fn start(config: OAuthProviderConfig) -> Result<AuthorizeState> {
    let client = OAuthClient::new(config).map_err(|e| {
        Error::Validation(ValidationError::Invalid(format!(
            "OAuth provider rejected: {e}"
        )))
    })?;
    let (mut url, state, verifier) = client.authorize_url()?;
    // `ferris-auth` hands the state back out of band; the browser must carry it
    // in the URL so the provider can echo it on the redirect.
    url.query_pairs_mut().append_pair("state", &state);
    Ok(AuthorizeState {
        url: url.to_string(),
        state,
        pkce_verifier: verifier,
    })
}

/// Swap the code the provider redirected back for tokens (async; hits the
/// configured token endpoint with `client_id`/`client_secret`/`redirect_uri`
/// and, when PKCE was used, the verifier).
///
/// # Errors
///
/// Fails when the provider configuration is rejected or the token endpoint
/// errors (bad code, expired verifier, transport).
pub async fn exchange_code(
    config: OAuthProviderConfig,
    code: String,
    pkce_verifier: Option<String>,
) -> Result<TokenResponse> {
    let client = OAuthClient::new(config).map_err(|e| {
        Error::Validation(ValidationError::Invalid(format!(
            "OAuth provider rejected: {e}"
        )))
    })?;
    client.exchange_code(code, pkce_verifier).await
}

/// Fetch the provider's user profile with `access_token` (RFC 6750 bearer).
///
/// # Errors
///
/// Fails when the provider configuration is rejected or the userinfo request
/// errors (expired token, transport).
pub async fn user_info(config: OAuthProviderConfig, access_token: &str) -> Result<Value> {
    let client = OAuthClient::new(config).map_err(|e| {
        Error::Validation(ValidationError::Invalid(format!(
            "OAuth provider rejected: {e}"
        )))
    })?;
    client.get_user_info(access_token).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::testing::{TestServer, exclusive, test_context};

    fn secret() -> SecretValue {
        SecretValue::new("csecret")
    }

    fn row(auth_url: &str, token_url: &str, with_redirect: bool) -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("provider_name".into(), serde_json::json!("mock"));
        obj.insert("client_id".into(), serde_json::json!("cid"));
        obj.insert("client_secret".into(), serde_json::json!("csecret"));
        obj.insert("authorization_uri".into(), serde_json::json!(auth_url));
        obj.insert("token_uri".into(), serde_json::json!(token_url));
        obj.insert("userinfo_uri".into(), serde_json::json!(""));
        obj.insert("scopes".into(), serde_json::json!("openid email"));
        if with_redirect {
            obj.insert(
                "redirect_uri".into(),
                serde_json::json!("http://localhost/cb"),
            );
        }
        Value::from_json(serde_json::Value::Object(obj))
    }

    #[test]
    fn config_requires_the_critical_fields() {
        let doc = row(
            "https://auth.example/authorize",
            "https://auth.example/token",
            true,
        );
        let cfg = provider_config(&doc, Some(&secret())).expect("valid config");
        assert_eq!(cfg.name, "mock");
        assert_eq!(cfg.client_id, "cid");
        assert_eq!(cfg.scopes, vec!["openid", "email"]);
        assert_eq!(cfg.redirect_uris, vec!["http://localhost/cb"]);

        let missing = row("", "https://auth.example/token", true);
        assert!(provider_config(&missing, Some(&secret())).is_err());

        let no_redirect = row("https://a/x", "https://a/t", false);
        assert!(provider_config(&no_redirect, Some(&secret())).is_err());
    }

    #[test]
    fn authorize_url_embeds_client_redirect_state_and_pkce() {
        let cfg = provider_config(
            &row(
                "https://auth.example/authorize",
                "https://auth.example/token",
                true,
            ),
            Some(&secret()),
        )
        .unwrap();
        let start = start(cfg).expect("authorize url");
        assert!(start.url.starts_with("https://auth.example/authorize"));
        assert!(start.url.contains("client_id=cid"));
        assert!(start.url.contains("redirect_uri="));
        assert!(start.url.contains("state="));
        assert!(start.url.contains("code_challenge="), "{}", start.url);
        assert!(
            start.url.contains("code_challenge_method=S256"),
            "{}",
            start.url
        );
        assert!(start.pkce_verifier.is_some(), "PKCE is on by default");
    }

    #[test]
    fn a_pkce_custom_field_can_opt_a_provider_out() {
        let mut doc = row(
            "https://auth.example/authorize",
            "https://auth.example/token",
            true,
        );
        if let Value::Object(fields) = &mut doc {
            fields.insert("pkce".to_string(), Value::Bool(false));
        }
        let cfg = provider_config(&doc, Some(&secret())).unwrap();
        let start = start(cfg).expect("authorize url");
        assert!(!start.url.contains("code_challenge="));
        assert!(start.pkce_verifier.is_none());
    }

    #[tokio::test]
    async fn exchange_code_posts_the_authorization_code() {
        let server = TestServer::start(
            200,
            r#"{"access_token":"at-123","token_type":"Bearer","expires_in":3600}"#,
        );
        let cfg = provider_config(
            &row(
                "https://auth.example/authorize",
                &format!("{}/token", server.url),
                true,
            ),
            Some(&secret()),
        )
        .unwrap();

        let tokens = exchange_code(cfg, "the-code".to_string(), None)
            .await
            .expect("token exchange");

        assert_eq!(tokens.access_token, "at-123");
        assert_eq!(tokens.token_type, "Bearer");

        let requests = server.received();
        assert_eq!(requests.len(), 1);
        let req = &requests[0];
        assert_eq!(req.method, "POST");
        assert!(req.path.starts_with("/token"));
        let body = req.body_text();
        assert!(body.contains("grant_type=authorization_code"), "{body}");
        assert!(body.contains("code=the-code"), "{body}");
        assert!(body.contains("client_id=cid"), "{body}");
        assert!(
            body.contains("redirect_uri=http%3A%2F%2Flocalhost%2Fcb"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn token_endpoint_errors_surface() {
        let server = TestServer::start(401, r#"{"error":"invalid_client"}"#);
        let cfg = provider_config(
            &row(
                "https://auth.example/authorize",
                &format!("{}/token", server.url),
                true,
            ),
            Some(&secret()),
        )
        .unwrap();
        assert!(exchange_code(cfg, "bad".to_string(), None).await.is_err());
    }

    #[test]
    fn a_flow_only_accepts_the_state_it_minted() {
        let cfg = provider_config(
            &row(
                "https://auth.example/authorize",
                "https://auth.example/token",
                true,
            ),
            Some(&secret()),
        )
        .unwrap();
        let flow = start(cfg).expect("authorize url");
        assert!(flow.accepts_state(&flow.state));
        assert!(!flow.accepts_state("another-flow"));
        assert!(!flow.accepts_state(""));
    }

    #[tokio::test]
    async fn complete_rejects_a_mismatched_state_before_the_token_call() {
        let cfg = provider_config(
            &row(
                "https://auth.example/authorize",
                "https://127.0.0.1:1/token",
                true,
            ),
            Some(&secret()),
        )
        .unwrap();
        let flow = start(cfg.clone()).expect("authorize url");
        // The token endpoint is unreachable; a state mismatch must fail before
        // any network call, so the error is a validation one.
        let error = flow
            .complete(cfg, "code".to_string(), "wrong-state")
            .await
            .expect_err("a mismatched state must be rejected");
        assert!(matches!(error, Error::Validation(_)), "{error:?}");
    }

    /// INT-01: the runtime reads enabled `Social Login Key` rows instead of a
    /// static provider list; a disabled row is skipped.
    #[tokio::test]
    async fn enabled_social_login_keys_are_loaded_from_the_doctype() {
        let _exclusive = exclusive().await;
        let ctx = test_context().await;

        for (name, enabled) in [("pkce-live", true), ("pkce-off", false)] {
            store::create(
                &ctx,
                SOCIAL_LOGIN_KEY,
                Value::from_json(serde_json::json!({
                    "provider_name": name,
                    "client_id": "cid",
                    "client_secret": "csecret",
                    "authorization_uri": "https://auth.example/authorize",
                    "token_uri": "https://auth.example/token",
                    "redirect_uri": "http://localhost/cb",
                    "enabled": enabled,
                })),
            )
            .await
            .expect("provider row");
        }

        let configs = load_social_login_keys(&ctx).await.expect("providers");
        let ours: Vec<_> = configs
            .iter()
            .filter(|config| config.name == "pkce-live" || config.name == "pkce-off")
            .collect();
        assert_eq!(ours.len(), 1, "only the enabled row is loaded");
        assert_eq!(ours[0].name, "pkce-live");
        assert!(ours[0].pkce_enabled, "PKCE is on by default");
    }

    /// The H3 regression for OAuth: the stored secret is encrypted, the document
    /// read only carries the mask, and `load_provider` is the path that gets the
    /// plaintext back to `OAuthProviderConfig` (whose `Debug` redacts it).
    #[tokio::test]
    async fn load_provider_decrypts_the_stored_client_secret() {
        use super::super::names::SOCIAL_LOGIN_KEY;

        let _exclusive = exclusive().await;
        let ctx = test_context().await;
        let created = store::create(
            &ctx,
            SOCIAL_LOGIN_KEY,
            Value::from_json(serde_json::json!({
                "provider_name": "mock",
                "client_id": "cid",
                "client_secret": "csecret",
                "authorization_uri": "https://auth.example/authorize",
                "token_uri": "https://auth.example/token",
                "redirect_uri": "http://localhost/cb",
            })),
        )
        .await
        .expect("provider row");
        let name = created
            .get("name")
            .and_then(Value::as_str)
            .expect("row name")
            .to_string();

        let config = load_provider(&ctx, SOCIAL_LOGIN_KEY, &name)
            .await
            .expect("provider config");
        assert_eq!(config.client_id, "cid");
        assert_eq!(config.client_secret, "csecret");
        assert!(
            !format!("{config:?}").contains("csecret"),
            "the config's Debug must stay redacted"
        );

        let masked = store::get(&ctx, SOCIAL_LOGIN_KEY, &name)
            .await
            .expect("document read");
        assert_eq!(
            masked.get("client_secret").and_then(Value::as_str),
            Some("********"),
            "an ordinary read never hands out the plaintext"
        );
    }
}
