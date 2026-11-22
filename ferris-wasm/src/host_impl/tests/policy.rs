//! Host policy: query scan cap, outbound-HTTP limits and pubsub delivery.

use super::super::http::{
    DEFAULT_HTTP_MAX_BODY_BYTES, HttpPolicy, MAX_HTTP_TIMEOUT_MS, clamp_timeout_ms,
};
use super::super::pubsub::publish_via;
use super::super::query::ensure_scan_within_cap;
use crate::host::{HostError, RealtimeBroadcaster, classify_host_error};
use std::sync::Arc;

/// M: `timeout-ms` is clamped to the host ceiling, including the default
/// the operator sets through the environment.
#[test]
fn http_timeout_is_clamped_to_the_host_ceiling() {
    assert_eq!(clamp_timeout_ms(None, 10_000), 10_000);
    assert_eq!(clamp_timeout_ms(Some(0), 10_000), 1);
    assert_eq!(clamp_timeout_ms(Some(5_000), 10_000), 5_000);
    assert_eq!(
        clamp_timeout_ms(Some(u32::MAX), 10_000),
        MAX_HTTP_TIMEOUT_MS
    );
    assert_eq!(clamp_timeout_ms(None, u64::MAX), MAX_HTTP_TIMEOUT_MS);
}

/// M: a post-processed query whose candidate set overflows the scan cap is
/// refused as a validation failure instead of returning a truncated total.
#[test]
fn scan_cap_refuses_a_truncated_candidate_set() {
    assert!(ensure_scan_within_cap(3, 3).is_ok());
    let err = ensure_scan_within_cap(4, 3).expect_err("over the cap");
    assert!(
        matches!(classify_host_error(&err), HostError::ValidationFailed(_)),
        "the guest must receive validation-failed (422), got {:?}",
        classify_host_error(&err)
    );
}

/// R-FW-2: the outbound-HTTP body cap defaults to the raised constant and
/// is enforced on the request before any network call.
#[test]
fn http_policy_raises_the_default_body_cap_and_enforces_it() {
    assert_eq!(DEFAULT_HTTP_MAX_BODY_BYTES, 16 * 1024 * 1024);

    let policy = HttpPolicy {
        allowed_hosts: vec!["example.com".to_string()],
        max_body_bytes: DEFAULT_HTTP_MAX_BODY_BYTES,
        default_timeout_ms: 1_000,
        client: reqwest::Client::new(),
    };

    // A body exactly at the cap passes; one byte over is refused.
    let at_cap = vec![0u8; DEFAULT_HTTP_MAX_BODY_BYTES];
    assert!(
        policy
            .policy_check("example.com", "POST", &[], Some(&at_cap), false, false)
            .is_ok()
    );
    let over = vec![0u8; DEFAULT_HTTP_MAX_BODY_BYTES + 1];
    assert!(
        policy
            .policy_check("example.com", "POST", &[], Some(&over), false, false)
            .is_err()
    );
}

#[tokio::test]
async fn pubsub_publish_without_a_broadcaster_is_an_error() {
    let error = publish_via(None, "doc:Task", "created", "{\"name\":\"T-1\"}")
        .expect_err("no broadcaster must not look like delivery");
    assert!(
        error
            .to_string()
            .contains("no realtime broadcaster registered"),
        "{error}"
    );
}

#[tokio::test]
async fn pubsub_publish_reaches_a_registered_broadcaster() {
    use parking_lot::Mutex;

    struct Recording(Arc<Mutex<Vec<(String, String, String)>>>);
    impl RealtimeBroadcaster for Recording {
        fn publish(&self, channel: &str, event_type: &str, payload: &str) -> anyhow::Result<()> {
            self.0.lock().push((
                channel.to_string(),
                event_type.to_string(),
                payload.to_string(),
            ));
            Ok(())
        }
    }

    let seen = Arc::new(Mutex::new(Vec::new()));
    let broadcaster: Arc<dyn RealtimeBroadcaster> = Arc::new(Recording(seen.clone()));
    publish_via(
        Some(broadcaster),
        "doc:Task",
        "created",
        "{\"name\":\"T-1\"}",
    )
    .expect("the broadcaster accepts the event");

    let seen = seen.lock();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "doc:Task");
    assert_eq!(seen[0].1, "created");
    assert_eq!(seen[0].2, "{\"name\":\"T-1\"}");
}
