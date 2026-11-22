//! `streams` behaviour: carry-over state and recreation signals.

use super::helpers::{memory_pool, test_host};
use crate::host::StreamInfo;

/// R-1: the per-stream cursor round-trips and is released when the stream
/// closes, so a reused id never inherits the previous tail.
#[tokio::test]
async fn stream_state_round_trips_and_is_released_on_close() {
    let host = test_host(memory_pool().await);
    host.register_stream("s-1", StreamInfo::new("websocket"));

    assert!(
        host.streams_state("s-1").await.expect("state").is_empty(),
        "a fresh stream starts with no carry-over"
    );
    host.streams_set_state("s-1", b"partial")
        .await
        .expect("set");
    assert_eq!(
        host.streams_state("s-1").await.expect("state"),
        b"partial".to_vec()
    );

    // An empty write clears the buffer.
    host.streams_set_state("s-1", b"").await.expect("clear");
    assert!(host.streams_state("s-1").await.expect("state").is_empty());

    // Closing drops the buffer and the id.
    host.streams_set_state("s-1", b"tail").await.expect("set");
    host.unregister_stream("s-1");
    assert!(
        host.streams_state("s-1").await.is_err(),
        "a closed stream has no state"
    );
}

/// H4: re-registering the same id from a different connection (a different
/// verified client certificate) is a recreation — the age and the
/// `registration-id` are re-stamped so a guest can tell it apart and drop
/// what it cached, and the identity is the one that connection presented,
/// never the previous connection's.
#[tokio::test]
async fn register_stream_restamps_recreation_signal_and_does_not_inherit_identity() {
    use ferris_core::ClientIdentity;

    fn identity(cn: &str) -> ClientIdentity {
        ClientIdentity {
            subject: format!("O=Solmec,CN={cn}"),
            common_name: Some(cn.to_string()),
            fingerprint: "ab".repeat(32),
            issuer: "O=Solmec".to_string(),
        }
    }

    let host = test_host(memory_pool().await);
    host.register_stream(
        "s-1",
        StreamInfo::new("rest").client_identity(Some(identity("A"))),
    );
    let first = host.streams_info("s-1").await.expect("info");
    assert_eq!(
        first
            .client_identity
            .as_ref()
            .and_then(|i| i.common_name.clone()),
        Some("A".to_string())
    );

    // Same identity: a continuation, so the age is preserved.
    host.register_stream(
        "s-1",
        StreamInfo::new("rest").client_identity(Some(identity("A"))),
    );
    let same = host.streams_info("s-1").await.expect("info");
    assert_eq!(
        same.created_at, first.created_at,
        "the same connection keeps the age"
    );
    assert_eq!(
        same.registration_id, first.registration_id,
        "a continuation of the same connection keeps the registration id"
    );

    // Different identity: a recreation re-stamps the age and takes the new
    // identity. A short sleep guarantees the millisecond clock advances.
    std::thread::sleep(std::time::Duration::from_millis(5));
    host.register_stream(
        "s-1",
        StreamInfo::new("rest").client_identity(Some(identity("B"))),
    );
    let recreated = host.streams_info("s-1").await.expect("info");
    assert_ne!(
        recreated.created_at, first.created_at,
        "a recreation with a different identity re-stamps the age"
    );
    assert_ne!(
        recreated.registration_id, first.registration_id,
        "a recreation by another connection re-stamps the registration id"
    );
    assert_eq!(
        recreated
            .client_identity
            .as_ref()
            .and_then(|i| i.common_name.as_deref()),
        Some("B")
    );

    // A later connection that presents no certificate must not inherit the
    // previous identity (that is how a clientless caller used to spoof the
    // first device).
    host.register_stream("s-1", StreamInfo::new("rest"));
    let anonymous = host.streams_info("s-1").await.expect("info");
    assert!(
        anonymous.client_identity.is_none(),
        "identity is not inherited across connections"
    );
    assert_ne!(
        anonymous.created_at, first.created_at,
        "losing the identity is also a recreation"
    );
    assert_ne!(
        anonymous.registration_id, recreated.registration_id,
        "losing the identity re-stamps the registration id"
    );
}
