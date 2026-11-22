//! `JobStore` behaviour: round trip, exactly-once claim, retry/backoff
//! columns, heartbeat-guarded recovery and the guarded update.

use super::*;
use crate::job::{Job, JobPriority, JobResult, JobStatus};
use chrono::Utc;
use ferris_core::Value;
use ferris_db::Stmt;

async fn store() -> JobStore {
    let store = JobStore::new(ferris_db::memory_db().await.unwrap());
    store.init().await.unwrap();
    store
}

fn job(queue: &str, priority: JobPriority) -> Job {
    Job {
        id: uuid::Uuid::new_v4().to_string(),
        name: "send_email".to_string(),
        queue: queue.to_string(),
        payload: ferris_core::Value::from("payload"),
        priority,
        status: JobStatus::Pending,
        retries: 0,
        max_retries: 3,
        timeout: std::time::Duration::from_secs(300),
        scheduled_at: None,
        started_at: None,
        completed_at: None,
        depends_on: vec!["job-a".to_string()],
        tags: vec!["nightly".to_string()],
        metadata: Default::default(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        worker_id: None,
        result: None,
    }
}

#[tokio::test]
async fn a_job_survives_the_round_trip() {
    let store = store().await;
    let mut original = job("default", JobPriority::High);
    original.result = Some(JobResult {
        success: true,
        output: Some(ferris_core::Value::from("done")),
        error: None,
        started_at: Utc::now(),
        completed_at: Utc::now(),
        duration_ms: 5,
    });
    store.persist(&original).await.unwrap();

    let loaded = store.get(&original.id).await.unwrap().unwrap();
    assert_eq!(loaded.name, "send_email");
    assert_eq!(loaded.id, original.id);
    assert_eq!(loaded.payload.as_str(), Some("payload"));
    assert_eq!(loaded.priority, JobPriority::High);
    assert_eq!(loaded.depends_on, vec!["job-a".to_string()]);
    assert_eq!(loaded.tags, vec!["nightly".to_string()]);
    assert_eq!(loaded.result.as_ref().map(|r| r.success), Some(true));
    // Nothing was set on the way in, so nothing may be invented on the way out.
    assert!(loaded.worker_id.is_none());
}

#[tokio::test]
async fn persisting_twice_replaces_rather_than_duplicates() {
    let store = store().await;
    let mut original = job("default", JobPriority::Normal);
    store.persist(&original).await.unwrap();

    original.status = JobStatus::Success;
    original.worker_id = Some("worker-1".to_string());
    store.persist(&original).await.unwrap();

    let listed = store.list("default", None, 10).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].status, JobStatus::Success);
    assert_eq!(listed[0].worker_id.as_deref(), Some("worker-1"));

    assert!(
        store
            .list("default", Some(JobStatus::Pending), 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_job_is_claimed_by_exactly_one_worker() {
    let store = store().await;
    store
        .persist(&job("default", JobPriority::Normal))
        .await
        .unwrap();

    let first = store
        .claim("worker-1", "default", Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.worker_id.as_deref(), Some("worker-1"));

    // The second worker must find nothing: the row is `running` now, which
    // the guarded update refuses.
    assert!(
        store
            .claim("worker-2", "default", Utc::now())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn claims_come_in_priority_then_age_order() {
    let store = store().await;
    let old = Utc::now() - chrono::Duration::minutes(5);
    let mut low = job("default", JobPriority::Low);
    low.created_at = old;
    let mut high = job("default", JobPriority::High);
    high.created_at = Utc::now();
    store.persist(&low).await.unwrap();
    store.persist(&high).await.unwrap();

    let claimed = store
        .claim("worker-1", "default", Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.priority, JobPriority::High);
}

#[tokio::test]
async fn a_future_job_is_not_claimed_yet() {
    let store = store().await;
    let mut pending = job("default", JobPriority::Normal);
    pending.scheduled_at = Some(Utc::now() + chrono::Duration::hours(1));
    store.persist(&pending).await.unwrap();

    assert!(
        store
            .claim("worker-1", "default", Utc::now())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .claim(
                "worker-1",
                "default",
                Utc::now() + chrono::Duration::hours(2)
            )
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_stalled_running_job_is_returned_to_the_queue() {
    let store = store().await;
    let mut stalled = job("default", JobPriority::Normal);
    stalled.status = JobStatus::Running;
    stalled.worker_id = Some("worker-gone".to_string());
    stalled.updated_at = Utc::now() - chrono::Duration::hours(2);
    store.persist(&stalled).await.unwrap();

    assert_eq!(
        store
            .recover_orphaned(60, "dead_letter")
            .await
            .unwrap()
            .len(),
        1
    );

    let reclaimed = store
        .claim("worker-2", "default", Utc::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reclaimed.worker_id.as_deref(), Some("worker-2"));

    // Nothing is left to recover.
    assert_eq!(
        store
            .recover_orphaned(60, "dead_letter")
            .await
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn an_orphan_that_used_up_its_retries_goes_to_the_dead_letter_queue() {
    let store = store().await;
    let mut stale = job("default", JobPriority::Normal);
    stale.status = JobStatus::Running;
    stale.worker_id = Some("worker-gone".to_string());
    stale.retries = 2; // one more recovery reaches the 3-retry cap
    stale.updated_at = Utc::now() - chrono::Duration::hours(2);
    store.persist(&stale).await.unwrap();

    assert_eq!(
        store
            .recover_orphaned(60, "dead_letter")
            .await
            .unwrap()
            .len(),
        1
    );

    let recovered = store.get(&stale.id).await.unwrap().unwrap();
    assert_eq!(recovered.status, JobStatus::DeadLetter);
    assert_eq!(recovered.retries, 3);
    // JOB11: the row left the source queue for the configured DLQ.
    assert_eq!(recovered.queue, "dead_letter");

    // A dead-lettered job is never claimed again.
    assert!(
        store
            .claim("worker-2", "default", Utc::now())
            .await
            .unwrap()
            .is_none()
    );
}

/// JOB6: the column update and the `payload` mirror must come from one
/// snapshot. Reintroduce the old drift (the column allows more retries than
/// the payload) and prove recovery believes the payload and writes the same
/// values back to both.
#[tokio::test]
async fn recovery_derives_columns_and_payload_from_one_snapshot() {
    let store = store().await;
    let mut stale = job("default", JobPriority::Normal);
    stale.status = JobStatus::Running;
    stale.worker_id = Some("worker-gone".to_string());
    stale.retries = 2; // the payload's max_retries is 3
    stale.updated_at = Utc::now() - chrono::Duration::hours(2);
    store.persist(&stale).await.unwrap();

    // The old recovery consulted this column for the retry cap while the
    // mirror consulted the payload; make them disagree.
    store
        .db
        .execute(&Stmt::new(
            format!("UPDATE {TAB_JOBS} SET max_retries = 5 WHERE id = ?"),
            vec![Value::from(stale.id.as_str())],
        ))
        .await
        .unwrap();

    assert_eq!(
        store
            .recover_orphaned(60, "dead_letter")
            .await
            .unwrap()
            .len(),
        1
    );

    let recovered = store.get(&stale.id).await.unwrap().unwrap();
    assert_eq!(recovered.status, JobStatus::DeadLetter);
    assert_eq!(recovered.retries, 3);
    assert_eq!(recovered.queue, "dead_letter");
    assert_eq!(recovered.worker_id, None);

    let row = store
        .db
        .fetch_one(&Stmt::new(
            format!("SELECT status, retries, queue, worker_id FROM {TAB_JOBS} WHERE id = ?"),
            vec![Value::from(stale.id.as_str())],
        ))
        .await
        .unwrap();
    assert_eq!(row.str("status").unwrap(), "dead_letter");
    assert_eq!(row.i64("retries").unwrap(), 3);
    assert_eq!(row.str("queue").unwrap(), "dead_letter");
    assert_eq!(row.opt_str("worker_id"), None);
}

/// JOB5: `update_guarded` is the primitive that stops two writers from
/// overwriting each other. The loser must get `false`, not silence.
#[tokio::test]
async fn a_guarded_update_refuses_a_stale_expected_status() {
    let store = store().await;
    let original = job("default", JobPriority::Normal);
    store.persist(&original).await.unwrap();

    let mut first = store.get(&original.id).await.unwrap().unwrap();
    first.status = JobStatus::Success;
    assert!(
        store
            .update_guarded(&first, JobStatus::Pending)
            .await
            .unwrap()
    );

    let mut second = first.clone();
    second.status = JobStatus::Cancelled;
    assert!(
        !store
            .update_guarded(&second, JobStatus::Pending)
            .await
            .unwrap()
    );

    let stored = store.get(&original.id).await.unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Success);
}

/// A15: recovery used to judge by `updated_at`, frozen at claim time, so a
/// healthy long-running job — its worker still heartbeating fine — was put
/// back on the queue and executed twice. A live heartbeat must protect the
/// job no matter how long it has been running.
#[tokio::test]
async fn a_running_job_with_a_heartbeating_worker_is_not_orphaned() {
    let store = store().await;
    let mut long_running = job("default", JobPriority::Normal);
    long_running.status = JobStatus::Running;
    long_running.worker_id = Some("worker-live".to_string());
    long_running.updated_at = Utc::now() - chrono::Duration::hours(2);
    store.persist(&long_running).await.unwrap();

    store
        .heartbeat("worker-live", "running", &["default".to_string()], 1)
        .await
        .unwrap();

    assert_eq!(
        store
            .recover_orphaned(60, "dead_letter")
            .await
            .unwrap()
            .len(),
        0
    );

    // Still running, still owned by the same worker; nothing was requeued.
    let still = store.get(&long_running.id).await.unwrap().unwrap();
    assert_eq!(still.status, JobStatus::Running);
    assert_eq!(still.worker_id.as_deref(), Some("worker-live"));
}

/// A15: when the worker's heartbeat goes stale past the cutoff, the job it
/// was running is recovered — the heartbeat was written at start, so only
/// a dead worker leaves it in the past.
#[tokio::test]
async fn a_job_of_a_worker_whose_heartbeat_stops_is_recovered() {
    let store = store().await;
    let mut stuck = job("default", JobPriority::Normal);
    stuck.status = JobStatus::Running;
    stuck.worker_id = Some("worker-dead".to_string());
    stuck.updated_at = Utc::now() - chrono::Duration::hours(2);
    store.persist(&stuck).await.unwrap();

    store
        .heartbeat("worker-dead", "running", &["default".to_string()], 1)
        .await
        .unwrap();
    // Backdate the heartbeat: the worker died two hours ago and stopped
    // writing. The claim (updated_at) is equally old.
    let old = (Utc::now() - chrono::Duration::hours(2)).timestamp();
    store
        .db
        .execute(&Stmt::new(
            "UPDATE job_workers SET last_heartbeat = ? WHERE worker_id = ?",
            vec![Value::from(old), Value::from("worker-dead")],
        ))
        .await
        .unwrap();

    assert_eq!(
        store
            .recover_orphaned(60, "dead_letter")
            .await
            .unwrap()
            .len(),
        1
    );

    let reclaimed = store.get(&stuck.id).await.unwrap().unwrap();
    assert_eq!(reclaimed.status, JobStatus::Retrying);
    assert_eq!(reclaimed.retries, 1);
}

#[tokio::test]
async fn terminal_counts_are_grouped_by_status() {
    let store = store().await;
    for status in [
        JobStatus::Success,
        JobStatus::Success,
        JobStatus::DeadLetter,
    ] {
        let mut j = job("default", JobPriority::Normal);
        j.status = status;
        store.persist(&j).await.unwrap();
    }
    // A different queue must not be counted.
    store
        .persist(&job("other", JobPriority::Normal))
        .await
        .unwrap();

    let counts = store.terminal_counts("default").await.unwrap();
    let find = |name: &str| {
        counts
            .iter()
            .find(|(status, _)| status == name)
            .map_or(0, |(_, count)| *count)
    };
    assert_eq!(find("success"), 2);
    assert_eq!(find("dead_letter"), 1);
    assert_eq!(find("pending"), 0);
}

#[tokio::test]
async fn a_heartbeat_replaces_the_previous_one() {
    let store = store().await;
    let queues = vec!["default".to_string()];
    store
        .heartbeat("worker-1", "busy", &queues, 3)
        .await
        .unwrap();
    store
        .heartbeat("worker-1", "idle", &queues, 0)
        .await
        .unwrap();

    let row = store
        .db
        .fetch_one(&Stmt::new(
            format!("SELECT status, current_jobs FROM {TAB_JOB_WORKERS} WHERE worker_id = ?"),
            vec![Value::from("worker-1")],
        ))
        .await
        .unwrap();
    assert_eq!(row.str("status").unwrap(), "idle");
    assert_eq!(row.i64("current_jobs").unwrap(), 0);
}

/// `history_retention_days` only ever removes terminal rows that finished
/// before the cutoff: a pending job and a recently finished one survive.
#[tokio::test]
async fn purging_history_removes_only_old_terminal_jobs() {
    let store = store().await;

    let mut old_success = job("default", JobPriority::Normal);
    old_success.status = JobStatus::Success;
    old_success.completed_at = Some(Utc::now() - chrono::Duration::days(90));
    let mut old_failure = job("default", JobPriority::Normal);
    old_failure.status = JobStatus::Failed;
    old_failure.completed_at = Some(Utc::now() - chrono::Duration::days(90));
    let mut recent_success = job("default", JobPriority::Normal);
    recent_success.status = JobStatus::Success;
    recent_success.completed_at = Some(Utc::now() - chrono::Duration::hours(1));
    let pending = job("default", JobPriority::Normal);

    for job in [&old_success, &old_failure, &recent_success, &pending] {
        store.persist(job).await.unwrap();
    }

    let purged = store
        .purge_terminal_before(Utc::now() - chrono::Duration::days(30))
        .await
        .unwrap();

    assert_eq!(purged, 2, "only the two old terminal rows are pruned");
    assert!(store.get(&old_success.id).await.unwrap().is_none());
    assert!(store.get(&old_failure.id).await.unwrap().is_none());
    assert!(store.get(&recent_success.id).await.unwrap().is_some());
    assert!(store.get(&pending.id).await.unwrap().is_some());

    // The dependency edges of the pruned rows go with them.
    let row = store
        .db
        .fetch_one(&Stmt::new(
            format!("SELECT COUNT(*) AS count FROM {TAB_JOB_DEPENDENCIES}"),
            vec![],
        ))
        .await
        .unwrap();
    assert_eq!(
        row.count("count"),
        2,
        "the two surviving jobs keep one edge each"
    );
}
