//! Shared HTTP client for the integrations' outbound calls.

use std::time::Duration;

/// Build the shared [`reqwest::Client`] every runtime module dispatches out of.
///
/// Conservative by default: a 10s connect + request timeout bounds how long a
/// document hook (webhooks, automation) or a scheduled job (Google sync) may
/// hold up its caller, and redirects are followed so a provider can move an
/// endpoint without breaking an integration. A caller that needs a stricter or
/// laxer profile can build its own client and hand it over through
/// [`super::state::IntegrationsContext::with_http`].
///
/// # Panics
///
/// Panics only if the default-safe builder profile (an unlikely combination of
/// settings) is rejected.
#[must_use]
pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(5))
        .tcp_nodelay(true)
        .build()
        .expect("a default-safe reqwest client")
}
