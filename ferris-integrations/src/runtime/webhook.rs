//! Outbound webhooks: turn a `Webhook` row into an HTTP call.
//!
//! A hook configured for `(doctype, event)` fires on the matching document
//! event ([`hooks::install`] mounts it). Payload and headers are small
//! templates rendered against the document ([`super::template`]); when
//! `webhook_secret` is set the body is signed with HMAC-SHA256 and sent as
//! `X-Ferris-Webhook-Signature` (hex).
//!
//! Delivery is best-effort on the save path: a slow or failing endpoint must
//! not strand the document write behind it, so a failed dispatch is logged (and
//! returned) but the hook swallows it. Deliveries through [`run_for_event`] are
//! sequential to keep per-dispatch ordering deterministic.

use std::collections::HashMap;

use ferris_core::secret::SecretValue;
use ferris_core::{Error, JobError, Result, ValidationError, Value};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::condition;
use super::names::WEBHOOK;
use super::state::{IntegrationsContext, ValueMap};
use super::{store, template};

/// The immutable part of a `Webhook` row the caller can dispatch by.
///
/// `webhook_secret` is a [`SecretValue`], not a `String`: the struct derives
/// `Debug` (and is logged on failure), so a plaintext secret here would be one
/// `{:?}` away from a log line. `signature` is the only reader.
///
/// Not `Clone` on purpose — cloning the spec would duplicate the plaintext, and
/// nothing needs a second copy.
#[derive(Debug)]
pub struct WebhookSpec {
    pub request_url: String,
    pub request_method: String,
    pub request_headers: String,
    pub request_body: String,
    pub webhook_secret: Option<SecretValue>,
    pub enabled: bool,
    pub condition: Option<String>,
}

/// One delivered webhook response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookOutcome {
    pub status: u16,
}

fn str_field(doc: &Value, key: &str) -> String {
    doc.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn bool_field(doc: &Value, key: &str, default: bool) -> bool {
    doc.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// Parse a stored `Webhook` row. Rows without a URL are skipped.
///
/// `secret` comes from [`store::secrets`] (the row itself only ever carries the
/// read mask, see `ferris_document::DocumentEngine::read_secret`). Passing
/// `None` means "no signing secret configured".
#[must_use]
pub fn parse_spec(doc: &Value, secret: Option<SecretValue>) -> Option<WebhookSpec> {
    let request_url = str_field(doc, "request_url");
    if request_url.trim().is_empty() {
        return None;
    }
    let condition = str_field(doc, "condition");
    Some(WebhookSpec {
        request_url,
        request_method: str_field(doc, "request_method"),
        request_headers: str_field(doc, "request_headers"),
        request_body: str_field(doc, "request_body"),
        webhook_secret: secret.filter(|value| !value.is_empty()),
        enabled: bool_field(doc, "enabled", true),
        condition: (!condition.is_empty()).then_some(condition),
    })
}

/// Whether `spec` should run for this event/doc: enabled, condition passes.
#[must_use]
pub fn matches(spec: &WebhookSpec, fields: &ValueMap, doctype: &str) -> bool {
    if !spec.enabled {
        return false;
    }
    match condition::evaluate_optional(spec.condition.as_deref(), fields) {
        Ok(true) => true,
        Ok(false) => false,
        Err(e) => {
            tracing::warn!(
                doctype,
                error = %e,
                "webhook condition failed to evaluate; skipping"
            );
            false
        }
    }
}

/// Render the request body: the whole document when `request_body` is blank,
/// otherwise the rendered template (kept as JSON when it parses, raw text when
/// it does not).
#[must_use]
pub fn build_body(spec: &WebhookSpec, doctype: &str, docname: &str, fields: &ValueMap) -> Vec<u8> {
    if spec.request_body.trim().is_empty() {
        let payload: serde_json::Value = {
            let mut map = serde_json::Map::new();
            for (k, v) in fields {
                map.insert(k.clone(), v.to_json());
            }
            serde_json::Value::Object(map)
        };
        return serde_json::to_vec(&payload).unwrap_or_default();
    }
    let rendered = template::render(&spec.request_body, doctype, docname, fields);
    match serde_json::from_str::<serde_json::Value>(&rendered) {
        Ok(value) => serde_json::to_vec(&value).unwrap_or_else(|_| rendered.into_bytes()),
        Err(_) => rendered.into_bytes(),
    }
}

/// HMAC-SHA256 signature over `body` as lowercase hex; `None` when no secret is
/// configured.
#[must_use]
pub fn signature(secret: Option<&SecretValue>, body: &[u8]) -> Option<String> {
    let secret = secret?.expose();
    if secret.is_empty() {
        return None;
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(body);
    Some(hex_encode(mac.finalize().into_bytes().as_slice()))
}

/// Parse the `request_headers` JSON object into a key/value list (invalid JSON
/// yields no custom headers).
#[must_use]
pub fn parse_headers(raw: &str) -> Vec<(String, String)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    value
        .as_object()
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Dispatch `spec` against the current document. Returns the response status.
///
/// `Err` covers transport failures and HTTP >= 400 responses so callers can
/// surface a failed delivery; the hook path logs and continues.
///
/// # Errors
///
/// Returns `Err` when the request cannot be built (invalid method/URL), the
/// transport fails, or the endpoint answers with an HTTP >= 400 status.
pub async fn dispatch(
    state: &IntegrationsContext,
    spec: &WebhookSpec,
    doctype: &str,
    docname: &str,
    fields: &ValueMap,
) -> Result<WebhookOutcome> {
    let method = if spec.request_method.trim().is_empty() {
        reqwest::Method::POST
    } else {
        reqwest::Method::from_bytes(spec.request_method.as_bytes()).map_err(|e| {
            Error::Validation(ValidationError::Invalid(format!(
                "invalid webhook request method: {e}"
            )))
        })?
    };
    let url = spec.request_url.trim().to_string();
    let body = build_body(spec, doctype, docname, fields);

    let mut request = state
        .http
        .request(method, &url)
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    for (key, value) in parse_headers(&spec.request_headers) {
        if let Ok(value) = reqwest::header::HeaderValue::from_str(&value) {
            request = request.header(key, value);
        }
    }
    if let Some(sig) = signature(spec.webhook_secret.as_ref(), &body) {
        request = request.header("X-Ferris-Webhook-Signature", sig);
    }
    request = request.body(body);

    let response = request.send().await.map_err(|e| {
        Error::Job(JobError::Failed(format!(
            "webhook request to `{url}` failed: {e}"
        )))
    })?;
    let status = response.status();
    if status.is_client_error() || status.is_server_error() {
        return Err(Error::Job(JobError::Failed(format!(
            "webhook request to `{url}` returned {status}"
        ))));
    }
    Ok(WebhookOutcome {
        status: status.as_u16(),
    })
}

/// Fire every enabled `Webhook` row configured for `(doctype, event)` against
/// the document `docname`/`fields`.
///
/// Best-effort per row: one failing endpoint is logged and the remaining
/// webhooks still run. Returns how many were dispatched.
///
/// # Errors
///
/// Fails only when the configured `Webhook` rows cannot be listed; per-webhook
/// dispatch failures are logged and do not stop the others.
pub async fn run_for_event(
    state: &IntegrationsContext,
    doctype: &str,
    event: &str,
    docname: &str,
    fields: &ValueMap,
) -> Result<usize> {
    if !state.features.webhooks {
        // Defense in depth: the hook layer already skips this call. A future
        // caller that reaches the dispatcher directly still respects the
        // operator's flag (zero rows dispatched, not a failure).
        return Ok(0);
    }
    let rows = store::list(
        state,
        WEBHOOK,
        HashMap::from([
            ("webhook_doctype".to_string(), Value::string(doctype)),
            ("webhook_docevent".to_string(), Value::string(event)),
        ]),
    )
    .await?;

    let mut dispatched = 0;
    for row in rows {
        let name = str_field(&row, "name");
        // The row read above returns the mask for `webhook_secret`; the real
        // value comes from the engine's revealed reader. A row whose secret
        // cannot be recovered (an Argon2 hash written before the field was
        // migrated, a retired key) is skipped with a loud warning, so one
        // broken webhook does not silence the others.
        let secret = match store::secrets(state, WEBHOOK, &name, &["webhook_secret"]).await {
            Ok(mut secrets) => secrets.remove("webhook_secret"),
            Err(e) => {
                tracing::warn!(
                    doctype,
                    docname = %name,
                    error = %e,
                    "webhook secret cannot be recovered; skipping this webhook"
                );
                continue;
            }
        };
        let Some(spec) = parse_spec(&row, secret) else {
            continue;
        };
        if !matches(&spec, fields, doctype) {
            continue;
        }
        match dispatch(state, &spec, doctype, docname, fields).await {
            Ok(outcome) => {
                dispatched += 1;
                tracing::info!(
                    doctype,
                    docname,
                    url = %spec.request_url,
                    status = outcome.status,
                    "webhook delivered"
                );
            }
            Err(e) => {
                tracing::warn!(doctype, docname, url = %spec.request_url, error = %e, "webhook failed");
            }
        }
    }
    Ok(dispatched)
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> HashMap<String, Value> {
        HashMap::from([
            ("name".to_string(), Value::string("INV-0001")),
            ("title".to_string(), Value::string("Invoice 42")),
            ("amount".to_string(), Value::from(1000)),
        ])
    }

    fn spec() -> WebhookSpec {
        WebhookSpec {
            request_url: "https://example.test/hook".into(),
            request_method: "POST".into(),
            request_headers: r#"{"X-Custom": "yes"}"#.into(),
            request_body: String::new(),
            webhook_secret: Some(SecretValue::new("s3cret")),
            enabled: true,
            condition: None,
        }
    }

    #[test]
    fn default_payload_is_the_whole_document() {
        let body = build_body(&spec(), "Sales Invoice", "INV-0001", &fields());
        let parsed: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(parsed["title"], "Invoice 42");
        assert_eq!(parsed["amount"], 1000);
    }

    #[test]
    fn templated_body_interpolates_and_stays_json() {
        let mut s = spec();
        s.request_body = r#"{"name": "{{ doc.title }}", "total": {{ doc.amount }}}"#.into();
        let body = build_body(&s, "X", "1", &fields());
        let parsed: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(parsed["name"], "Invoice 42");
        assert_eq!(parsed["total"], 1000);
    }

    #[test]
    fn signature_is_deterministic_and_secret_dependent() {
        let body = b"hello";
        let secret = SecretValue::new("secret");
        let other = SecretValue::new("other");
        let a = signature(Some(&secret), body).expect("signed");
        let b = signature(Some(&secret), body).expect("signed");
        let c = signature(Some(&other), body);
        assert_eq!(a, b);
        assert_ne!(a, c.unwrap());
        assert_eq!(a.len(), 64, "sha256 hex");
        assert!(signature(None, body).is_none());
        assert!(signature(Some(&SecretValue::new("")), body).is_none());
    }

    /// The reason H3 existed: a `Password` declaration made this field read back
    /// as `"********"`, so every delivery was signed with the mask. The spec's
    /// secret must survive `Debug` redacted, too.
    #[test]
    fn parsing_a_row_keeps_the_secret_out_of_debug_and_signs_with_it() {
        let doc = Value::from_json(serde_json::json!({
            "name": "HOOK-1",
            "request_url": "https://example.test/hook",
            "webhook_secret": "********",
        }));
        let spec = parse_spec(&doc, Some(SecretValue::new("s3cret"))).expect("spec");
        assert!(!format!("{spec:?}").contains("s3cret"), "{spec:?}");
        assert_eq!(
            signature(spec.webhook_secret.as_ref(), b"body"),
            signature(Some(&SecretValue::new("s3cret")), b"body")
        );

        // No configured secret is an unsigned delivery, not an empty signature.
        let unsigned = parse_spec(&doc, None).expect("spec");
        assert!(unsigned.webhook_secret.is_none());
        assert!(signature(unsigned.webhook_secret.as_ref(), b"body").is_none());
    }

    #[test]
    fn headers_parse_only_strings_and_ignore_garbage() {
        let headers = parse_headers(r#"{"X-A": "1", "X-B": 2}"#);
        assert_eq!(headers, vec![("X-A".to_string(), "1".to_string())]);
        assert!(parse_headers("not json").is_empty());
    }

    #[test]
    fn disabled_or_condition_blocked_webhooks_do_not_match() {
        let mut s = spec();
        s.enabled = false;
        assert!(!matches(&s, &fields(), "X"));

        let mut s = spec();
        s.condition = Some("amount > 500".into());
        assert!(matches(&s, &fields(), "X"));
        s.condition = Some("amount > 5000".into());
        assert!(!matches(&s, &fields(), "X"));
    }
}
