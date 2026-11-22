//! Outbound host allow-list for app-owned sockets.
//!
//! The stream transport reuses the same vocabulary the HTTP path enforces
//! (`FERRIS_HTTP_ALLOWED_HOSTS`, exact host or `*.suffix`; loopback and private
//! ranges refused unless the call was granted `allow-private`), because an app
//! opening a raw socket to a host it could not call over HTTP would be a hole
//! with a different protocol on it.
//!
//! The list is resolved **once per connect**: a name that points at a private
//! address is refused even when the name itself is allow-listed, which is the
//! check a static host list cannot make.

use std::net::{IpAddr, SocketAddr};

use anyhow::Result;

/// The outbound policy of one stream connection.
#[derive(Debug, Clone)]
pub struct EgressPolicy {
    allowed_hosts: Vec<String>,
    /// Whether the app holds `http-unrestricted`: skip the allow-list, keep the
    /// private-address check unless the call also asks for `allow-private`.
    unrestricted: bool,
    /// Whether this call may reach loopback/private targets.
    allow_private: bool,
}

impl EgressPolicy {
    /// The operator allow-list, read from the same variable the HTTP client
    /// uses. An empty value means "nothing", not "everything": a policy that
    /// fails open on a typo is not a policy.
    pub fn from_env(unrestricted: bool, allow_private: bool) -> Self {
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
        Self {
            allowed_hosts,
            unrestricted,
            allow_private,
        }
    }

    /// Resolve and authorise `host:port`, returning the addresses to try.
    ///
    /// # Errors
    ///
    /// The host is outside the allow-list, resolves to a private address without
    /// `allow-private`, or does not resolve at all.
    pub async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        if !self.unrestricted && !host_in(&self.allowed_hosts, host) {
            anyhow::bail!(
                "stream target `{host}` is not in the outbound allow-list \
                 (FERRIS_HTTP_ALLOWED_HOSTS) and this app does not hold `http-unrestricted`"
            );
        }
        let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| anyhow::anyhow!("cannot resolve `{host}:{port}`: {e}"))?
            .collect();
        if addresses.is_empty() {
            anyhow::bail!("`{host}:{port}` resolved to no address");
        }
        if !self.allow_private {
            for address in &addresses {
                if ip_is_private(address.ip()) {
                    anyhow::bail!(
                        "stream target `{host}` resolves to the private address {} and \
                         `allow-private` is not granted to this app",
                        address.ip()
                    );
                }
            }
        }
        Ok(addresses)
    }

    /// Whether this call may reach `ip` when there is no name to check against
    /// the allow-list (a raw/ICMP destination, or a datagram peer the host
    /// already parsed).
    #[must_use]
    pub fn allows_address(&self, ip: IpAddr) -> bool {
        self.allow_private || !ip_is_private(ip)
    }
}

/// Whether `host` is a wildcard bind (`0.0.0.0`, `::`, or empty): binding it
/// publishes the socket on every interface, which is the exposure
/// [`crate::plugin_net::streams`] refuses unless the app holds the grant.
pub fn is_wildcard_bind(host: &str) -> bool {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    host.is_empty() || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_unspecified())
}

/// Whether `host` is named by an allow-list entry: exact match or `*.suffix`.
pub fn host_in(allowed: &[String], host: &str) -> bool {
    let host = host.to_lowercase();
    allowed.iter().any(|entry| {
        let entry = entry.trim().to_lowercase();
        entry.strip_prefix("*.").map_or_else(
            || host == entry,
            |suffix| host == suffix || host.ends_with(&format!(".{suffix}")),
        )
    })
}

/// Whether an address is outside the public unicast ranges.
pub fn ip_is_private(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || (a == 100 && (64..=127).contains(&b))
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| ip_is_private(v4.into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_entries_respect_label_boundaries() {
        let allowed = vec!["api.test".to_string(), "*.internal.test".to_string()];
        assert!(
            host_in(&allowed, "API.test"),
            "matching is case-insensitive"
        );
        assert!(host_in(&allowed, "node.internal.test"));
        assert!(!host_in(&allowed, "api.test.evil.test"));
        assert!(!host_in(&allowed, "evil.test"));
    }

    #[test]
    fn private_addresses_are_recognised() {
        for raw in ["127.0.0.1", "10.0.0.1", "192.168.0.1", "169.254.1.1", "::1"] {
            assert!(ip_is_private(raw.parse().unwrap()), "{raw}");
        }
        for raw in ["93.184.216.34", "2001:4860:4860::8888"] {
            assert!(!ip_is_private(raw.parse().unwrap()), "{raw}");
        }
    }

    #[tokio::test]
    async fn an_allow_listed_name_that_points_at_loopback_is_refused() {
        let policy = EgressPolicy::from_env(false, false);
        // `localhost` is on the default allow-list but resolves to loopback,
        // which still needs `allow-private`.
        let err = policy
            .resolve("localhost", 9)
            .await
            .expect_err("loopback needs the grant even when allow-listed");
        assert!(err.to_string().contains("private address"), "{err}");

        let allowed = EgressPolicy::from_env(false, true);
        assert!(
            allowed.resolve("localhost", 9).await.is_ok(),
            "the grant lets it through"
        );
    }

    #[tokio::test]
    async fn a_host_outside_the_list_is_refused_before_resolving() {
        let policy = EgressPolicy::from_env(false, false);
        let err = policy
            .resolve("example.invalid", 80)
            .await
            .expect_err("not allow-listed");
        assert!(err.to_string().contains("allow-list"), "{err}");
    }

    /// A datagram or raw target is an address, not a name: the allow-list has
    /// nothing to match, but the private-address rule still applies.
    #[test]
    fn a_bare_address_keeps_the_private_rule() {
        let denied = EgressPolicy::from_env(false, false);
        assert!(!denied.allows_address("127.0.0.1".parse().expect("ip")));
        assert!(!denied.allows_address("10.0.0.7".parse().expect("ip")));
        assert!(denied.allows_address("93.184.216.34".parse().expect("ip")));
        let granted = EgressPolicy::from_env(true, true);
        assert!(granted.allows_address("127.0.0.1".parse().expect("ip")));
    }

    #[test]
    fn only_the_unspecified_address_is_a_wildcard_bind() {
        for host in ["0.0.0.0", "::", "[::]", ""] {
            assert!(is_wildcard_bind(host), "{host}");
        }
        for host in ["127.0.0.1", "::1", "192.0.2.10"] {
            assert!(!is_wildcard_bind(host), "{host}");
        }
    }
}
