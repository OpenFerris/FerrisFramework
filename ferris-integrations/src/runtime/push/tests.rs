//! Push delivery tests: in-app fan-out, retry bookkeeping and exhaustion.

use std::sync::{Arc, Mutex, OnceLock};

use ferris_core::Value;

use super::*;
use crate::runtime::names::{NOTIFICATION, PUSH_NOTIFICATION};
use crate::runtime::state::IntegrationsContext;
use crate::runtime::store;
use crate::runtime::testing::{exclusive, test_context};

type PublishSink = Arc<Mutex<Vec<(String, Value)>>>;

/// One process-wide publisher (the real `set_publisher` is first-call-wins).
fn published() -> &'static PublishSink {
    static SINK: OnceLock<PublishSink> = OnceLock::new();
    let sink = SINK.get_or_init(|| Arc::new(Mutex::new(Vec::new())));
    let forward = Arc::clone(sink);
    let _ = ferris_core::notifications::set_publisher(Arc::new(move |user, notification| {
        forward
            .lock()
            .expect("publish sink")
            .push((user.to_string(), notification));
    }));
    sink
}

async fn push_row(ctx: &IntegrationsContext, user: Option<&str>) -> String {
    let mut data = serde_json::json!({
        "title": "Deploy failed",
        "body": "The nightly build broke.",
        "status": "Queued",
    });
    // The `user` field is a Link: an empty string is rejected on insert, so a
    // row without a target simply omits the key (the delivery then fails with
    // the `Required` validation this suite wants).
    if let Some(user) = user {
        data["user"] = serde_json::Value::String(user.to_string());
    }
    let row = store::create(ctx, PUSH_NOTIFICATION, Value::from_json(data))
        .await
        .expect("push row");
    row.get("name")
        .and_then(Value::as_str)
        .expect("name")
        .to_string()
}

async fn row_by_name(ctx: &IntegrationsContext, name: &str) -> Value {
    store::get(ctx, PUSH_NOTIFICATION, name)
        .await
        .expect("push row")
}

#[tokio::test]
async fn a_queued_row_reaches_in_app_exactly_once() {
    let _exclusive = exclusive().await;
    let ctx = test_context().await;
    let sink = published();
    sink.lock().expect("sink").clear();

    let name = push_row(&ctx, Some("ada@example.com")).await;
    assert_eq!(deliver_pending(&ctx).await.expect("sweep"), 1);

    let row = row_by_name(&ctx, &name).await;
    assert_eq!(str_field(&row, "status"), "Sent");
    assert_eq!(int_field(&row, "attempts"), 1);
    assert!(!str_field(&row, "sent_at").is_empty());
    assert!(str_field(&row, "next_attempt_at").is_empty());

    let notifications = store::list(&ctx, NOTIFICATION, std::collections::HashMap::new())
        .await
        .expect("notifications");
    assert_eq!(notifications.len(), 1);
    assert_eq!(
        notifications[0].get("user").and_then(Value::as_str),
        Some("ada@example.com")
    );
    assert_eq!(sink.lock().expect("sink").len(), 1);

    // The next sweep must not deliver it again.
    assert_eq!(deliver_pending(&ctx).await.expect("sweep"), 0);
    assert_eq!(
        store::list(&ctx, NOTIFICATION, std::collections::HashMap::new())
            .await
            .expect("notifications")
            .len(),
        1
    );
    assert_eq!(sink.lock().expect("sink").len(), 1);
}

#[tokio::test]
async fn a_failed_row_is_retried_after_its_backoff_and_then_succeeds() {
    let _exclusive = exclusive().await;
    let ctx = test_context().await;
    let sink = published();
    sink.lock().expect("sink").clear();

    // Empty target: the delivery fails deterministically before reaching the
    // in-app feed.
    let name = push_row(&ctx, None).await;
    let policy = PushRetryPolicy {
        max_attempts: 3,
        base_delay_secs: 0,
    };
    let error = deliver_with_policy(&ctx, &name, policy)
        .await
        .expect_err("no target user");
    assert!(error.to_string().contains("user"), "{error}");

    let row = row_by_name(&ctx, &name).await;
    assert_eq!(str_field(&row, "status"), "Failed");
    assert_eq!(int_field(&row, "attempts"), 1);
    assert!(str_field(&row, "error").contains("user"));
    assert_eq!(
        store::list(&ctx, NOTIFICATION, std::collections::HashMap::new())
            .await
            .expect("notifications")
            .len(),
        0,
        "a failure must not create an in-app row"
    );

    // Fix the row and let the (zero) backoff elapse: the retry delivers.
    store::update(
        &ctx,
        PUSH_NOTIFICATION,
        &name,
        Value::from_json(serde_json::json!({ "user": "ada@example.com" })),
    )
    .await
    .expect("fix user");
    deliver_with_policy(&ctx, &name, policy)
        .await
        .expect("retry succeeds");

    let row = row_by_name(&ctx, &name).await;
    assert_eq!(str_field(&row, "status"), "Sent");
    assert_eq!(int_field(&row, "attempts"), 2);
    assert_eq!(
        store::list(&ctx, NOTIFICATION, std::collections::HashMap::new())
            .await
            .expect("notifications")
            .len(),
        1
    );
    assert_eq!(sink.lock().expect("sink").len(), 1);
}

#[tokio::test]
async fn a_row_stops_retrying_when_the_budget_is_exhausted() {
    let _exclusive = exclusive().await;
    let ctx = test_context().await;
    let name = push_row(&ctx, None).await;
    let policy = PushRetryPolicy {
        max_attempts: 2,
        base_delay_secs: 0,
    };
    for _ in 0..2 {
        deliver_with_policy(&ctx, &name, policy)
            .await
            .expect_err("still failing");
    }
    let row = row_by_name(&ctx, &name).await;
    assert_eq!(int_field(&row, "attempts"), 2);
    assert_eq!(str_field(&row, "status"), "Failed");

    // Further calls are a no-op, not a reset of the counter.
    deliver_with_policy(&ctx, &name, policy)
        .await
        .expect("exhausted rows are skipped");
    assert_eq!(int_field(&row_by_name(&ctx, &name).await, "attempts"), 2);
}
