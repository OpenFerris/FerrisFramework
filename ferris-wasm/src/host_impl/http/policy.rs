//! The outbound allow-list, the connect-time resolver and the per-call client.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::redirect::Attempt;

/// Connect-time DNS guard: one resolution per connection, and the addresses
/// that were checked are the addresses that get dialled.
///
/// `resolve_and_check_private` runs before the request so a bad target fails
/// with a readable policy error, but it resolves the name *separately* from the
/// connection. Between the two, DNS can answer differently (rebinding), and a
/// redirect opens a connection the pre-flight check never saw. Installing this
/// resolver on every client closes both gaps at the only place the decision is
/// final: the moment a socket address is chosen for a name (WASM-016).
pub(super) struct GuardedResolver {
    /// The effective `allow-private` of the call: the grant *and* the flag.
    pub(super) allow_private: bool,
}

/// The error half of [`reqwest::dns::Resolving`]; reqwest does not re-export
/// the alias, so it is spelled here once.
type DnsError = Box<dyn std::error::Error + Send + Sync>;

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let allow_private = self.allow_private;
        Box::pin(async move {
            let host = name.as_str().to_string();
            // Port `0`: reqwest replaces it with the URL's port, which is what
            // keeps a resolved address usable across redirects.
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| Box::new(error) as DnsError)?
                .collect();
            if addrs.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("`{host}` resolved to no address"),
                )) as DnsError);
            }
            if !allow_private
                && let Some(private) = addrs.iter().find(|addr| ip_is_private(addr.ip()))
            {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "`{host}` resolves to the private address {} and `allow-private` is not \
                         granted to this call",
                        private.ip()
                    ),
                )) as DnsError);
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Outbound-HTTP policy the host enforces on the `http::request` import: an
/// allow-list of hosts, size caps, a timeout and credential-header hygiene.
///
/// Configuration comes from the environment so a deployment can tighten it
/// without rebuilding: - `FERRIS_HTTP_ALLOWED_HOSTS` — comma-separated hosts (exact match or a
///   `.`-parent domain suffix). Defaults to `localhost,127.0.0.1`.
/// - `FERRIS_HTTP_MAX_BODY` — request/response body cap, bytes (default
///   [`DEFAULT_HTTP_MAX_BODY_BYTES`], 16 MiB).
/// - `FERRIS_HTTP_TIMEOUT_MS` — per-call timeout, milliseconds (default 10 s).
/// - `FERRIS_HTTP_ALLOW_PRIVATE` — whether the default allow-list may reach
///   loopback/RFC 1918 targets; today the default list is loopback-only on
///   purpose, so a deployment that needs a private target adds it explicitly.
pub struct HttpPolicy {
    pub allowed_hosts: Vec<String>,
    pub max_body_bytes: usize,
    pub default_timeout_ms: u64,
    /// Default client: follows up to 5 redirects, each re-checked.
    pub client: reqwest::Client,
}

/// Hard cap on redirect hops, whatever the guest asks for.
const MAX_REDIRECT_HOPS: u8 = 10;

/// A redirect policy that re-checks every hop against the outbound allow-list:
/// a `Location` pointing at a host the plugin could not call directly must not
/// be followed. Stops at `max_hops`.
fn redirect_policy(
    allowed: Vec<String>,
    max_hops: u8,
    allow_private: bool,
) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt: Attempt<'_>| {
        if attempt.previous().len() >= usize::from(max_hops) {
            return attempt.stop();
        }
        let allowed_target = attempt.url().host_str().is_some_and(|host| {
            if !allowed.is_empty() && !host_in(&allowed, host) {
                return false;
            }
            // A redirect to a literal private address is refused unless the
            // call was allowed to reach private targets; a name that *resolves*
            // to one is caught by the connect-time check in `policy_check`.
            allow_private || !host_is_private_literal(host)
        });
        if allowed_target {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

/// Whether `host` is named by a guest `allowed-hosts` entry: an exact host or
/// a `"*.suffix"` wildcard, the vocabulary `types.http-policy` documents.
pub(super) fn host_in(allowed: &[String], host: &str) -> bool {
    let host = host.to_lowercase();
    allowed.iter().any(|entry| {
        let entry = entry.trim().to_lowercase();
        entry.strip_prefix("*.").map_or_else(
            || host == entry,
            |suffix| host == suffix || host.ends_with(&format!(".{suffix}")),
        )
    })
}

/// Whether a literal address in `host` is loopback, RFC 1918, link-local or
/// otherwise not publicly routable. Names are not resolved here (that is
/// [`HttpPolicy::resolve_and_check_private`]); a name that *is* one of these
/// spellings is caught.
pub(super) fn host_is_private_literal(host: &str) -> bool {
    // `localhost` is a name, not an address, but it is the canonical private
    // target and the operator allow-list normally names it explicitly.
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|addr| ip_is_private(addr))
}

/// Whether an address is outside the public unicast ranges.
pub(super) fn ip_is_private(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                // 192.0.2.0/24, 198.51.100.0/24 and 203.0.113.0/24 (TEST-NET).
                || (a == 192 && b == 0 && v4.octets()[2] == 2)
                || (a == 198 && b == 51 && v4.octets()[2] == 100)
                || (a == 203 && b == 0 && v4.octets()[2] == 113)
                // 100.64.0.0/10 carrier-grade NAT.
                || (a == 100 && (64..=127).contains(&b))
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // Unique local (fc00::/7) and link-local (fe80::/10).
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || v6.to_ipv4_mapped().is_some_and(|v4| ip_is_private(v4.into()))
        }
    }
}

/// The request/response body cap a plugin gets when the operator does not set
/// `FERRIS_HTTP_MAX_BODY`.
///
/// The old 1 MiB default was small enough that ordinary artifact and report
/// transfers bounced off it; 16 MiB still bounds a single plugin call while
/// covering the common case, and operators who need more (or less) set the
/// variable. The cap is enforced while the response streams, so raising it does
/// not mean buffering an unbounded body first.
pub const DEFAULT_HTTP_MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Hard ceiling for one outbound HTTP call, milliseconds.
///
/// The guest may ask for less, never more: without this a `timeout-ms` of
/// `u32::MAX` (≈49 days) pins a runtime worker far past any request deadline.
/// The operator's default is clamped by the same value, so
/// `FERRIS_HTTP_TIMEOUT_MS` cannot raise the ceiling either.
pub const MAX_HTTP_TIMEOUT_MS: u64 = 60_000;

/// Resolve the effective per-call timeout.
///
/// A missing guest value falls back to the configured default; both are clamped
/// to `1..=MAX_HTTP_TIMEOUT_MS` so a zero is a short (not absent) timeout and a
/// pathological value cannot outlive the process.
pub fn clamp_timeout_ms(requested: Option<u32>, default: u64) -> u64 {
    requested
        .map_or(default, u64::from)
        .clamp(1, MAX_HTTP_TIMEOUT_MS)
}

impl HttpPolicy {
    pub fn from_env() -> Self {
        let allowed_hosts = std::env::var("FERRIS_HTTP_ALLOWED_HOSTS").map_or_else(
            |_| vec!["localhost".to_string(), "127.0.0.1".to_string()],
            |raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|host| !host.is_empty())
                    .map(str::to_string)
                    .collect()
            },
        );
        let max_body_bytes = std::env::var("FERRIS_HTTP_MAX_BODY")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_HTTP_MAX_BODY_BYTES);
        let default_timeout_ms = clamp_timeout_ms(
            std::env::var("FERRIS_HTTP_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok()),
            10_000,
        );
        // Hosts redirect a caller around the allow-list; a plugin that wants to
        // follow further hops must do so explicitly, and every hop is
        // re-checked against the allow-list by the custom policy below. The
        // resolver re-checks the private-address rule at every connection,
        // including the ones a redirect opens.
        let client = reqwest::Client::builder()
            .redirect(redirect_policy(allowed_hosts.clone(), 5, false))
            .dns_resolver(Arc::new(GuardedResolver {
                allow_private: false,
            }))
            .build()
            .unwrap_or_default();
        Self {
            allowed_hosts,
            max_body_bytes,
            default_timeout_ms,
            client,
        }
    }

    /// The redirect policy a call asked for: `none` | `limited` (default) |
    /// `follow`. An unknown mode is rejected (classified as validation-failed)
    /// rather than silently defaulting.
    pub(super) fn redirect_policy(
        &self,
        mode: Option<&str>,
        hops: Option<u8>,
    ) -> Result<reqwest::redirect::Policy> {
        match mode.map(str::to_ascii_lowercase).as_deref() {
            Some("none") => Ok(reqwest::redirect::Policy::none()),
            None | Some("limited") => Ok(redirect_policy(
                self.allowed_hosts.clone(),
                hops.unwrap_or(5).min(MAX_REDIRECT_HOPS),
                false,
            )),
            Some("follow") => Ok(redirect_policy(
                self.allowed_hosts.clone(),
                MAX_REDIRECT_HOPS,
                false,
            )),
            Some(other) => anyhow::bail!(
                "unknown redirect-policy `{other}` (use `none`, `limited` or `follow`)"
            ),
        }
    }

    /// The client for a call's redirect mode and private-target grant. Only the
    /// default client of an unprivileged call is shared; the others build one
    /// per call, because the redirect budget and the connect-time resolver
    /// cannot be changed on a built client.
    pub(super) fn client_for(
        &self,
        mode: Option<&str>,
        hops: Option<u8>,
        allow_private: bool,
    ) -> Result<reqwest::Client> {
        match mode.map(str::to_ascii_lowercase).as_deref() {
            None | Some("limited") if hops.is_none() && !allow_private => Ok(self.client.clone()),
            _ => Ok(reqwest::Client::builder()
                .redirect(self.redirect_policy(mode, hops)?)
                .dns_resolver(Arc::new(GuardedResolver { allow_private }))
                .build()
                .context("building the redirect-mode http client")?),
        }
    }

    /// Whether `host` (the URL's host, lowercased) is allowed: an entry
    /// matches exactly or as a `.`-suffixed parent domain.
    fn allows(&self, host: &str) -> bool {
        host_in(&self.allowed_hosts, host)
    }

    /// The body cap of one call: the strictest of the guest policy and the
    /// host's own ceiling.
    pub(super) fn effective_max_body(&self, requested: Option<u64>) -> usize {
        requested
            .and_then(|bytes| usize::try_from(bytes).ok())
            .map_or(self.max_body_bytes, |bytes| bytes.min(self.max_body_bytes))
    }

    /// Resolve `url`'s host and refuse a private target unless `allow_private`.
    ///
    /// This is what closes the gap between "host is on the allow-list" and
    /// "host points at 127.0.0.1": an allow-list entry can be a name, and a
    /// name's address is decided by DNS, not by the declaration.
    ///
    /// It is the readable half of the rule. The connection itself is guarded by
    /// [`GuardedResolver`], which resolves again at connect time and refuses a
    /// private answer there too, so a second DNS answer cannot race this check
    /// and a redirect's connection is checked as well.
    pub(super) async fn resolve_and_check_private(
        &self,
        url: &reqwest::Url,
        allow_private: bool,
    ) -> Result<()> {
        if allow_private {
            return Ok(());
        }
        let Some(host) = url.host_str() else {
            return Ok(());
        };
        if host_is_private_literal(host) {
            anyhow::bail!(
                "`{host}` is a private target and `allow-private` is not granted to this call"
            );
        }
        let port = url.port_or_known_default().unwrap_or(80);
        let Ok(addrs) = tokio::net::lookup_host((host, port)).await else {
            // An unresolvable name fails at connect time with a better error
            // than this check can produce.
            return Ok(());
        };
        for addr in addrs {
            if ip_is_private(addr.ip()) {
                anyhow::bail!(
                    "`{host}` resolves to the private address {} and `allow-private` is not \
                     granted to this call",
                    addr.ip()
                );
            }
        }
        Ok(())
    }

    /// The first address `host` resolves to, used to keep a connection at the
    /// original target while SNI names something else.
    pub(super) async fn resolve_first(
        &self,
        host: &str,
        port: u16,
    ) -> Result<std::net::SocketAddr> {
        tokio::net::lookup_host((host, port))
            .await
            .with_context(|| format!("resolving `{host}:{port}`"))?
            .next()
            .ok_or_else(|| anyhow::anyhow!("`{host}:{port}` resolved to no address"))
    }

    pub fn policy_check(
        &self,
        host: &str,
        method: &str,
        headers: &[(String, String)],
        body: Option<&[u8]>,
        unrestricted: bool,
        allow_credentials: bool,
    ) -> Result<()> {
        const ALLOWED_METHODS: [&str; 6] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"];
        const FORBIDDEN_HEADERS: [&str; 6] = [
            "host",
            "proxy-authorization",
            "proxy-authenticate",
            "x-forwarded-for",
            "x-forwarded-host",
            "x-forwarded-proto",
        ];
        // `http-unrestricted` skips the allow-list, never the caps: the method
        // list and the credential hygiene still apply.
        if !unrestricted && !self.allows(host) {
            anyhow::bail!(
                "host `{host}` is not in the outbound allow-list (FERRIS_HTTP_ALLOWED_HOSTS)"
            );
        }
        let upper = method.to_uppercase();
        if !ALLOWED_METHODS.contains(&upper.as_str()) {
            anyhow::bail!(
                "http method `{method}` is not allowed (GET, POST, PUT, PATCH, DELETE, HEAD)"
            );
        }
        // Credential-header hygiene: these carry host credentials or identity,
        // and a plugin must not be able to impersonate the server on a request
        // the host issues. Rejecting is honest; silently stripping would be
        // surprising. `Authorization`/`Cookie` are forwarded only when the app
        // declared `allow-credentials` and holds `http-unrestricted`.
        for (name, _) in headers {
            let lower = name.to_lowercase();
            let credential = lower == "authorization" || lower == "cookie";
            if credential && allow_credentials {
                continue;
            }
            if credential
                || FORBIDDEN_HEADERS
                    .iter()
                    .any(|forbidden| *forbidden == lower)
            {
                anyhow::bail!(
                    "header `{name}` is managed by the host and cannot be set by a plugin{}",
                    if credential {
                        " without `http-policy.allow-credentials`"
                    } else {
                        ""
                    }
                );
            }
        }
        if let Some(body) = body
            && body.len() > self.max_body_bytes
        {
            anyhow::bail!("http body exceeds the {} byte limit", self.max_body_bytes);
        }
        Ok(())
    }
}
