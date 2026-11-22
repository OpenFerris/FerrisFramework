//! Scheduler E2E over a real Redis server.
//!
//! Boots the [`Scheduler`] on a live Redis-backed `JobQueue`, registers a
//! per-second cron schedule and verifies it fires into the real `queue:<name>`
//! sorted set, persisting `next_run`. Skips silently when no Redis
//! server answers; CI starts a `redis:7` service container.

// Integration tests link the crate's whole dependency set but reference only a
// handful; the workspace's `unused_crate_dependencies` lint would otherwise fire
// once per unreferenced dependency.
#![allow(unused_crate_dependencies)]

use ferris_core::SchedulerConfig;
use ferris_db::memory_db;
use ferris_jobs::{JobQueue, QueueConfig};
use ferris_scheduler::{Schedule, ScheduleConfig, Scheduler, SchedulerHandle};
use redis::Client as RedisClient;
use std::sync::Arc;
use std::time::Duration;

/// Open a Redis client only when a server answers `PING` right now.
async fn redis_client() -> Option<RedisClient> {
    let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let client = RedisClient::open(url).ok()?;
    let mut conn = client.get_multiplexed_async_connection().await.ok()?;
    let pong: String = redis::cmd("PING").query_async(&mut conn).await.ok()?;
    if pong == "PONG" { Some(client) } else { None }
}

#[tokio::test]
async fn redis_scheduler_boots_cron_fires_and_persists_next_run() {
    let Some(client) = redis_client().await else {
        eprintln!("REDIS E2E: no redis server, skipping");
        return;
    };
    let unique = format!("e2e-sched-{}", uuid::Uuid::new_v4());
    let db = memory_db().await.expect("in-memory db");
    let queue = JobQueue::new(
        QueueConfig {
            name: unique.clone(),
            ..Default::default()
        },
        client,
    )
    .await
    .expect("job queue over redis");
    let queue = Arc::new(queue.with_db(db.clone()));
    queue.init().await.expect("queue init");

    let scheduler = Arc::new(
        // The default tick is 60s: a per-second cron would not fire inside this
        // test's 5s window. Set the cadence the schedule expects.
        Scheduler::with_config(
            queue.clone(),
            db.clone(),
            &SchedulerConfig {
                tick_interval_secs: 1,
                ..Default::default()
            },
        )
        .await
        .expect("scheduler"),
    );
    scheduler.start().await.expect("scheduler start");
    let handle = SchedulerHandle::new(scheduler);

    // Every second, for the duration of this test.
    let schedule = Schedule::new(
        "redis-e2e".to_string(),
        ScheduleConfig {
            cron_expression: "* * * * * *".to_string(),
            job_name: "e2e_cron".to_string(),
            queue: unique.clone(),
            ..Default::default()
        },
    );
    let schedule_id = handle.add_schedule(schedule).await.expect("add schedule");

    // Wait (up to 5s) until the cron has fired at least once into the real
    // Redis queue.
    let mut queued = 0usize;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let stats = queue.get_queue_stats(&unique).await.expect("queue stats");
        if stats.queued >= 1 {
            queued = stats.queued;
            break;
        }
    }
    assert!(
        queued >= 1,
        "cron schedule never fired into the redis queue"
    );

    // The cron closure rolled the deadline forward and persisted it (A12).
    let fetched = handle
        .get_schedule(&schedule_id)
        .await
        .expect("get schedule")
        .expect("schedule still present");
    assert!(
        fetched.next_run.is_some(),
        "next_run must be persisted after a run"
    );

    handle.shutdown().await.expect("scheduler shutdown");
}
