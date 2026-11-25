//! OAuth2 login wiring and the public origin absolute URLs are built from.

use tracing::warn;

use crate::server::ServerSettings;

/// project the operator's `AuthConfig.oauth` onto the auth crate's
/// OAuth configuration, ready to hand to [`OAuthManager::new`].
///
/// PKCE S256 is **forced** on regardless of the stored `pkce_enabled` flag:
/// the flow's one-time `state` + server-side verifier store already exists, and
/// a `plain`/disabled PKCE provider would ship a weaker authorization code
/// exchange for no benefit. Every provider also gets exactly one redirect URI
/// built from [`public_base_url`], so `/api/oauth/{name}/callback` always exists
/// even when the deployed config omits one (the old open-redirect behaviour of
/// deriving it from the provider URL is not re-introduced).
pub(super) fn oauth_config_from_settings(
    settings: &ServerSettings,
) -> ferris_core::Result<ferris_auth::OAuthConfig> {
    use ferris_auth::{PkceMethod, StateStoreConfig};
    let base = format!("{}/api/oauth", public_base_url(settings));
    let providers = settings
        .config
        .auth
        .oauth
        .providers
        .iter()
        .map(|p| ferris_auth::OAuthProviderConfig {
            name: p.name.clone(),
            client_id: p.client_id.clone(),
            client_secret: p.client_secret.clone(),
            auth_url: p.auth_url.clone(),
            token_url: p.token_url.clone(),
            user_info_url: (!p.user_info_url.is_empty()).then(|| p.user_info_url.clone()),
            device_auth_url: None,
            scopes: p.scopes.clone(),
            pkce_enabled: true,
            pkce_method: PkceMethod::S256,
            redirect_uris: vec![format!("{base}/{}/callback", p.name)],
            // The login path only trusts a provider-verified address; a
            // deployment whose provider omits `email_verified` has to opt out.
            require_verified_email: true,
            // This flow is OAuth2 + userinfo; an `id_token` is not validated.
            oidc: false,
        })
        .collect();
    Ok(ferris_auth::OAuthConfig {
        providers,
        state_store: StateStoreConfig {
            enabled: true,
            ttl_seconds: 600,
        },
    })
}

/// The public origin absolute URLs are built from (SRV2).
///
/// `server.external_url` wins when set. Otherwise fall back to the bind
/// `host:port` with the scheme implied by in-process TLS — correct for
/// `ferris dev`, wrong behind a reverse proxy, which is why the fallback warns
/// on a wildcard bind: a browser cannot reach `0.0.0.0`, and an OAuth provider
/// will reject the loopback callback the old code advertised.
pub(super) fn public_base_url(settings: &ServerSettings) -> String {
    let configured = settings
        .config
        .server
        .external_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty());
    if let Some(url) = configured {
        return url.trim_end_matches('/').to_string();
    }

    let scheme = if settings.config.server.tls.is_some() {
        "https"
    } else {
        "http"
    };
    if matches!(settings.host.trim(), "" | "0.0.0.0" | "::") {
        warn!(
            "server.external_url is not set and the server binds a wildcard address: \
             absolute URLs will advertise loopback. Set `[server] external_url` to the \
             public origin — OAuth providers reject the fallback callback."
        );
    }
    format!("{scheme}://{}:{}", base_host(settings), settings.port)
}

/// The configured bind host, in a form usable inside a callback URL. `0.0.0.0`
/// and `::` are not reachable client-side, so they map to loopback.
fn base_host(settings: &ServerSettings) -> String {
    match settings.host.trim() {
        "" | "0.0.0.0" | "::" => "127.0.0.1".to_string(),
        other => other.to_string(),
    }
}
