//! The push-sweep ownership rule lives in the parent module (it depends on the
//! role set, not on the email hub), so its test lives here.

use super::*;
use crate::roles::ServerRoles;

fn settings_with_roles(roles: &[&str]) -> ServerSettings {
    let roles: Vec<String> = roles.iter().map(|role| (*role).to_string()).collect();
    ServerSettings::new(
        "127.0.0.1".to_string(),
        0,
        ServerRoles::parse(&roles).expect("roles"),
        ferris_core::DatabaseConfig::default(),
        ferris_core::Config::default(),
    )
}

/// The scheduler owns the push sweep when it runs; a worker-only process has
/// no scheduler to enqueue the job and must sweep inline, or retries would
/// never happen.
#[test]
fn the_push_sweep_is_inline_only_without_a_scheduler() {
    assert!(push_sweep_is_inline(&settings_with_roles(&[
        "api", "worker"
    ])));
    assert!(!push_sweep_is_inline(&settings_with_roles(&[
        "api",
        "worker",
        "scheduler"
    ])));
}

/// SCHED-01: `start_scheduler` must hand the operator's `[scheduler]` table to
/// the instance, so the tick cadence, enqueue concurrency and retention window
/// are the effective ones (they used to be parsed, logged and ignored).
#[cfg(feature = "scheduler")]
#[tokio::test]
async fn start_scheduler_applies_the_configured_values() {
    let mut config = ferris_core::Config::default();
    config.scheduler.tick_interval_secs = 7;
    config.scheduler.max_concurrent_jobs = 3;
    config.scheduler.history_retention_days = 11;
    config.scheduler.timezone = "Europe/Madrid".to_string();

    let settings = ServerSettings::new(
        "127.0.0.1".to_string(),
        0,
        ServerRoles::parse(&["api".to_string(), "scheduler".to_string()]).expect("roles"),
        ferris_core::DatabaseConfig::default(),
        config,
    );

    let db = ferris_db::memory_db().await.expect("in-memory db");
    let queue = Arc::new(JobQueue::new_db(QueueConfig::default(), db.clone()));
    queue.init().await.expect("queue init");

    let scheduler = start_scheduler(&settings, &db, queue)
        .await
        .expect("scheduler starts");

    assert_eq!(scheduler.config().tick_interval_secs, 7);
    assert_eq!(scheduler.config().max_concurrent_jobs, 3);
    assert_eq!(scheduler.config().history_retention_days, 11);
    assert_eq!(scheduler.config().timezone, "Europe/Madrid");
    assert_eq!(scheduler.timezone().to_string(), "Europe/Madrid");

    scheduler.shutdown().await.expect("scheduler shutdown");
}
