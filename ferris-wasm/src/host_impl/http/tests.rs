//! Outbound-HTTP policy regressions: allow-list, private targets, caps.

use super::policy::{GuardedResolver, host_in, host_is_private_literal};
use super::*;

#[test]
fn a_guest_narrowed_allow_list_accepts_exact_and_wildcard_hosts_only() {
    let allowed = vec!["api.test".to_string(), "*.internal.test".to_string()];
    assert!(host_in(&allowed, "api.test"));
    assert!(host_in(&allowed, "node1.internal.test"));
    assert!(
        host_in(&allowed, "internal.test"),
        "the bare suffix matches"
    );
    assert!(!host_in(&allowed, "evil.test"));
    assert!(
        !host_in(&allowed, "api.test.evil.test"),
        "a suffix must be a label boundary"
    );
}

#[test]
fn private_targets_are_recognised_without_resolving_a_name() {
    for host in [
        "localhost",
        "127.0.0.1",
        "127.9.9.9",
        "10.1.2.3",
        "192.168.1.1",
        "172.16.0.1",
        "169.254.169.254",
        "::1",
        "fd00::1",
        "fe80::1",
        "100.64.0.1",
    ] {
        assert!(host_is_private_literal(host), "{host} must be private");
    }
    for host in ["93.184.216.34", "example.test", "2001:4860:4860::8888"] {
        assert!(!host_is_private_literal(host), "{host} must be public");
    }
}

#[test]
fn the_body_cap_is_the_strictest_of_the_call_and_the_host() {
    let policy = HttpPolicy {
        allowed_hosts: Vec::new(),
        max_body_bytes: 1024,
        default_timeout_ms: 1000,
        client: reqwest::Client::new(),
    };
    assert_eq!(policy.effective_max_body(None), 1024);
    assert_eq!(policy.effective_max_body(Some(10)), 10);
    assert_eq!(
        policy.effective_max_body(Some(1_000_000)),
        1024,
        "a guest cannot raise the host ceiling"
    );
}

/// The credential headers are refused unless the operator allowed them,
/// and the ones that name host/proxy identity are refused unconditionally —
/// `http-unrestricted` does not lift header hygiene.
#[test]
fn credential_headers_need_the_grant_and_proxy_headers_never_pass() {
    let policy = HttpPolicy {
        allowed_hosts: vec!["api.test".to_string()],
        max_body_bytes: 1024,
        default_timeout_ms: 1000,
        client: reqwest::Client::new(),
    };
    let auth = vec![("Authorization".to_string(), "Bearer t".to_string())];
    let err = policy
        .policy_check("api.test", "GET", &auth, None, true, false)
        .expect_err("authorization without the grant must be refused");
    assert!(err.to_string().contains("allow-credentials"), "{err}");
    policy
        .policy_check("api.test", "GET", &auth, None, true, true)
        .expect("the grant forwards it");

    let forwarded = vec![("x-forwarded-for".to_string(), "1.2.3.4".to_string())];
    let err = policy
        .policy_check("api.test", "GET", &forwarded, None, true, true)
        .expect_err("proxy identity is host-owned");
    assert!(err.to_string().contains("x-forwarded-for"), "{err}");
}

/// `unrestricted` is the only way past the allow-list, and it does not
/// widen the method set.
#[test]
fn the_allow_list_is_skipped_only_for_an_unrestricted_call() {
    let policy = HttpPolicy {
        allowed_hosts: vec!["api.test".to_string()],
        max_body_bytes: 1024,
        default_timeout_ms: 1000,
        client: reqwest::Client::new(),
    };
    assert!(
        policy
            .policy_check("elsewhere.test", "GET", &[], None, false, false)
            .is_err()
    );
    policy
        .policy_check("elsewhere.test", "GET", &[], None, true, false)
        .expect("unrestricted ignores the allow-list");
    assert!(
        policy
            .policy_check("elsewhere.test", "TRACE", &[], None, true, false)
            .is_err(),
        "the caps still apply"
    );
}

/// WASM-016: every connection resolves through the guard, so the address
/// that was checked is the address dialled, and a redirect's connection is
/// checked too. Without the grant, a name that points at loopback is refused
/// at connect time even if the pre-flight check saw a public answer.
#[tokio::test]
async fn the_connect_time_resolver_refuses_a_private_name() {
    use reqwest::dns::Resolve as _;
    use std::str::FromStr as _;

    let denied = GuardedResolver {
        allow_private: false,
    };
    let name = reqwest::dns::Name::from_str("localhost").expect("a valid name");
    let err = denied
        .resolve(name)
        .await
        .err()
        .expect("loopback is refused without the grant");
    assert!(err.to_string().contains("private address"), "{err}");

    let granted = GuardedResolver {
        allow_private: true,
    };
    let name = reqwest::dns::Name::from_str("localhost").expect("a valid name");
    let addrs = granted
        .resolve(name)
        .await
        .expect("the grant reaches loopback")
        .collect::<Vec<_>>();
    assert!(!addrs.is_empty(), "the grant still resolves real addresses");
    assert!(
        addrs.iter().all(|addr| addr.port() == 0),
        "the port is left to reqwest, so a redirect keeps its own"
    );
}
