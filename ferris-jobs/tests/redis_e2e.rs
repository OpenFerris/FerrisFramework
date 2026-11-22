//! Jobs queue E2E over a real Redis server.
//!
//! `JobQueue::new` pings Redis at construction, `enqueue` writes the job into
//! the `queue:<name>` sorted set and `get_queue_stats` reads it back. These tests
//! run whenever a Redis server answers on `REDIS_URL`
//! (`redis://127.0.0.1:6379` by default) and skip silently otherwise, so the
//! suite stays green on hosts without Redis. CI starts a `redis:7` service
//! container, so they also run there.

// Integration tests link the crate's whole dependency set but reference only a
// handful; the workspace's `unused_crate_dependencies` lint would otherwise fire
// once per unreferenced dependency.
#![allow(unused_crate_dependencies)]

use ferris_core::Value;
use ferris_db::memory_db;
use ferris_jobs::{Job, JobPriority, JobQueue, JobResult, QueueConfig};
use redis::Client as RedisClient;
use std::time::Duration;

/// Open a Redis client only when a server answers `PING` right now.
async fn redis_client() -> Option<RedisClient> {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let client = RedisClient::open(url).ok()?;
    let mut conn = client.get_multiplexed_async_connection().await.ok()?;
    let pong: String = redis::cmd("PING").query_async(&mut conn).await.ok()?;
    if pong == "PONG" { Some(client) } else { None }
}

/// Build a `JobQueue` on a live Redis server with a unique queue name and a
/// memory store. Callers must already hold a `redis_client()`.
async fn queue(client: RedisClient, unique: &str) -> JobQueue {
    let db = memory_db().await.expect("in-memory db");
    let config = QueueConfig {
        name: unique.to_string(),
        ..Default::default()
    };
    let queue = JobQueue::new(config, client)
        .await
        .expect("job queue over redis")
        .with_db(db);
    queue.init().await.expect("queue init");
    queue
}

/// A job enqueued on the given (unique) queue.
fn job(queue: &str) -> Job {
    let mut job = Job::new(format!("e2e:{queue}"), Value::Null);
    job.queue = queue.to_string();
    job
}

#[tokio::test]
async fn redis_round_trip_enqueue_claim_complete() {
    let Some(client) = redis_client().await else {
        eprintln!("REDIS E2E: no redis server, skipping");
        return;
    };
    let unique = format!("e2e-rt-{}", uuid::Uuid::new_v4());
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("redis conn");
    let queue = queue(client, &unique).await;

    let id = queue.enqueue(job(&unique)).await.expect("enqueue");

    // The Redis sorted set actually received the job.
    let zcard: i64 = redis::cmd("ZCARD")
        .arg(format!("queue:{unique}"))
        .query_async(&mut conn)
        .await
        .expect("zcard");
    assert_eq!(zcard, 1, "enqueue must land in the real Redis queue");

    let worker = format!("worker-{}", uuid::Uuid::new_v4());
    let claimed = queue
        .pop_job(&worker, Some(&unique))
        .await
        .expect("claim")
        .expect("claimed job");
    assert_eq!(claimed.id, id, "claimed job is the enqueued one");

    // A claimed job is waiting no longer: the sorted set is the `queued`
    // gauge, and `enqueue` used to ZADD without anything ever removing.
    let after_claim: i64 = redis::cmd("ZCARD")
        .arg(format!("queue:{unique}"))
        .query_async(&mut conn)
        .await
        .expect("zcard");
    assert_eq!(after_claim, 0, "a claimed job leaves the waiting set");

    // A second worker must not see the now-running job.
    let second = queue
        .pop_job(&format!("worker-{}", uuid::Uuid::new_v4()), Some(&unique))
        .await
        .expect("second claim");
    assert!(second.is_none(), "a running job is not double-delivered");

    queue
        .complete_job(&id, JobResult::success(Value::Null))
        .await
        .expect("complete");
    let stats = queue.get_queue_stats(&unique).await.expect("stats");
    assert!(
        stats.completed >= 1,
        "terminal state recorded against the store"
    );
    assert_eq!(stats.queued, 0, "a completed job is not waiting");
}

/// A failed job that still has retries left goes back to the waiting set (with
/// its backoff deadline as the score) and leaves it again when it is claimed.
#[tokio::test]
async fn redis_failed_job_returns_to_the_waiting_set() {
    let Some(client) = redis_client().await else {
        eprintln!("REDIS E2E: no redis server, skipping");
        return;
    };
    let unique = format!("e2e-retry-{}", uuid::Uuid::new_v4());
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("redis conn");
    let queue = queue(client, &unique).await;

    let id = queue
        .enqueue(job(&unique).with_max_retries(2))
        .await
        .expect("enqueue");
    queue
        .pop_job(&format!("worker-{}", uuid::Uuid::new_v4()), Some(&unique))
        .await
        .expect("claim")
        .expect("claimed");
    queue.fail_job(&id, "boom".to_string()).await.expect("fail");

    let zcard: i64 = redis::cmd("ZCARD")
        .arg(format!("queue:{unique}"))
        .query_async(&mut conn)
        .await
        .expect("zcard");
    assert_eq!(zcard, 1, "a retrying job waits for its backoff");
    assert_eq!(
        queue.get_queue_stats(&unique).await.expect("stats").queued,
        1,
        "the gauge counts the retrying job as waiting"
    );
}

/// Orphan recovery puts a reclaimed job back into the waiting set, so the
/// gauge keeps matching the durable queue.
#[tokio::test]
async fn redis_orphan_recovery_returns_the_job_to_the_waiting_set() {
    let Some(client) = redis_client().await else {
        eprintln!("REDIS E2E: no redis server, skipping");
        return;
    };
    let unique = format!("e2e-orphan-{}", uuid::Uuid::new_v4());
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("redis conn");
    let queue = queue(client, &unique).await;

    queue.enqueue(job(&unique)).await.expect("enqueue");
    queue
        .pop_job(&format!("worker-{}", uuid::Uuid::new_v4()), Some(&unique))
        .await
        .expect("claim")
        .expect("claimed");

    // A worker that never heartbeats and whose claim is already stale. The
    // stale window is compared in whole seconds (`updated_at < cutoff`), so
    // wait past the current second instead of racing the truncation.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let recovered = queue.recover_orphaned_jobs(0).await.expect("recover");
    assert_eq!(recovered, 1, "the orphaned job is reclaimed");

    let zcard: i64 = redis::cmd("ZCARD")
        .arg(format!("queue:{unique}"))
        .query_async(&mut conn)
        .await
        .expect("zcard");
    assert_eq!(zcard, 1, "a recovered job is waiting again");
    assert_eq!(
        queue.get_queue_stats(&unique).await.expect("stats").queued,
        1,
        "the gauge counts the recovered job as waiting"
    );
}

#[tokio::test]
async fn redis_priority_order_is_respected() {
    let Some(client) = redis_client().await else {
        eprintln!("REDIS E2E: no redis server, skipping");
        return;
    };
    let unique = format!("e2e-prio-{}", uuid::Uuid::new_v4());
    let queue = queue(client, &unique).await;
    let worker = format!("worker-{}", uuid::Uuid::new_v4());

    let mut low = job(&unique);
    low.priority = JobPriority::Normal;
    let mut high = job(&unique);
    high.priority = JobPriority::Critical;
    queue.enqueue(low).await.expect("enqueue normal");
    queue.enqueue(high).await.expect("enqueue critical");

    let first = queue
        .pop_job(&worker, Some(&unique))
        .await
        .expect("claim first")
        .expect("first job");
    let second = queue
        .pop_job(&worker, Some(&unique))
        .await
        .expect("claim second")
        .expect("second job");
    assert_eq!(
        first.priority,
        JobPriority::Critical,
        "highest priority leaves the queue first"
    );
    assert_eq!(second.priority, JobPriority::Normal);
}

#[tokio::test]
async fn redis_future_jobs_stay_queued_until_due() {
    let Some(client) = redis_client().await else {
        eprintln!("REDIS E2E: no redis server, skipping");
        return;
    };
    let unique = format!("e2e-future-{}", uuid::Uuid::new_v4());
    let queue = queue(client, &unique).await;

    let in_an_hour = chrono::Utc::now() + chrono::Duration::hours(1);
    let id = queue
        .enqueue_at(job(&unique), in_an_hour)
        .await
        .expect("enqueue_at");

    let popped = queue
        .pop_job(&format!("worker-{}", uuid::Uuid::new_v4()), Some(&unique))
        .await
        .expect("claim");
    assert!(popped.is_none(), "future jobs are not claimed early");

    queue.cancel_job(&id).await.expect("cancel");
}
