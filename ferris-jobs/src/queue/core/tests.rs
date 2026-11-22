//! `JobQueue` behaviour: backoff, dead-lettering, loud failures, counting and
//! the dependency gate.

use super::*;
use crate::JobPriority;

#[test]
fn test_queue_config() {
    let config = QueueConfig::default();
    assert_eq!(config.name, "default");
    assert_eq!(config.retry_backoff_base_secs, 60);
    assert_eq!(config.dead_letter_queue, "dead_letter");
}

/// JOB2/JOB11: a failed job's backoff is written to the `scheduled_at`
/// **column** the claim query reads, so the retry is not claimable
/// immediately. Before the fix the column stayed `NULL` and this returned
/// the job straight away.
#[tokio::test]
async fn a_failed_job_is_not_claimable_before_its_backoff() {
    let queue = JobQueue::new_db(
        QueueConfig {
            retry_backoff_base_secs: 60,
            ..Default::default()
        },
        ferris_db::memory_db().await.unwrap(),
    );
    queue.init().await.unwrap();

    let id = queue
        .enqueue(Job::new("app.task".to_string(), ferris_core::Value::Null).with_max_retries(2))
        .await
        .unwrap();

    let claimed = queue
        .pop_job("worker-1", None)
        .await
        .unwrap()
        .expect("the fresh job is claimable");
    assert_eq!(claimed.id, id);

    queue.fail_job(&id, "boom".to_string()).await.unwrap();

    let stored = queue.get_job(&id).await.unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Retrying);
    assert!(
        stored.scheduled_at.is_some(),
        "the retry must carry a backoff deadline"
    );

    assert!(
        queue.pop_job("worker-1", None).await.unwrap().is_none(),
        "a retrying job must wait for its backoff"
    );
}

/// JOB11: exhausting the retries moves the row to the configured DLQ, not
/// just to the `dead_letter` status inside its source queue.
#[tokio::test]
async fn an_exhausted_job_moves_to_the_dead_letter_queue() {
    let queue = JobQueue::new_db(
        QueueConfig {
            dead_letter_queue: "dlq".to_string(),
            ..Default::default()
        },
        ferris_db::memory_db().await.unwrap(),
    );
    queue.init().await.unwrap();

    let id = queue
        .enqueue(Job::new("app.task".to_string(), ferris_core::Value::Null).with_max_retries(1))
        .await
        .unwrap();
    let claimed = queue.pop_job("worker-1", None).await.unwrap().unwrap();
    queue
        .fail_job(&claimed.id, "permanent".to_string())
        .await
        .unwrap();

    let stored = queue.get_job(&id).await.unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::DeadLetter);
    assert_eq!(stored.queue, "dlq");
    assert!(queue.pop_job("worker-1", None).await.unwrap().is_none());
    assert_eq!(queue.get_queue_stats("dlq").await.unwrap().dead_letter, 1);
}

/// JOB13: an unknown id is an error, not a silent success.
#[tokio::test]
async fn failing_an_unknown_job_reports_not_found() {
    let queue = JobQueue::new_db(
        QueueConfig::default(),
        ferris_db::memory_db().await.unwrap(),
    );
    queue.init().await.unwrap();

    let error = queue
        .fail_job("does-not-exist", "boom".to_string())
        .await
        .expect_err("an unknown job must not report success");
    assert!(matches!(
        error,
        Error::Job(JobError::NotFound(id)) if id == "does-not-exist"
    ));
}

/// JOB8: `processing` is the per-queue running count, not a map that is
/// never written.
#[tokio::test]
async fn processing_counts_the_running_jobs_of_that_queue() {
    let queue = JobQueue::new_db(
        QueueConfig::default(),
        ferris_db::memory_db().await.unwrap(),
    );
    queue.init().await.unwrap();
    queue
        .enqueue(Job::new("app.task".to_string(), ferris_core::Value::Null))
        .await
        .unwrap();
    assert_eq!(queue.get_stats().await.unwrap().processing, 0);

    queue.pop_job("worker-1", None).await.unwrap().unwrap();
    assert_eq!(queue.get_stats().await.unwrap().processing, 1);
    assert_eq!(queue.get_stats().await.unwrap().queued, 0);
}

/// JOB4: a queue with neither backend, or with Redis but no store, must not
/// pretend a claim found nothing.
#[tokio::test]
async fn a_storeless_queue_fails_loudly() {
    let queue = JobQueue::new_db(
        QueueConfig::default(),
        ferris_db::memory_db().await.unwrap(),
    );
    // `new_db` always has a store; simulate the half-built Redis-only shape
    // by taking it away.
    let mut queue = queue;
    queue.store = None;

    let error = queue
        .pop_job("worker-1", None)
        .await
        .expect_err("a storeless queue cannot claim");
    assert!(matches!(error, Error::Job(JobError::Connection(_))));
}

/// R-FW-1a: a database-only queue (no Redis at all) enqueues, reports the
/// waiting count, and hands the job to a worker.
#[tokio::test]
async fn a_db_only_queue_enqueues_claims_and_counts() {
    let queue = JobQueue::new_db(
        QueueConfig::default(),
        ferris_db::memory_db().await.unwrap(),
    );
    queue.init().await.unwrap();

    let id = queue
        .enqueue(Job::new(
            "app.task".to_string(),
            ferris_core::Value::from("payload"),
        ))
        .await
        .unwrap();

    assert_eq!(queue.get_stats().await.unwrap().queued, 1);

    let claimed = queue
        .pop_job("worker-1", None)
        .await
        .unwrap()
        .expect("the persisted job is claimable without Redis");
    assert_eq!(claimed.id, id);
    assert_eq!(claimed.status, JobStatus::Running);

    // Once claimed it is no longer waiting.
    assert_eq!(queue.get_stats().await.unwrap().queued, 0);
}

/// `depends_on` used to be persisted and then ignored by the claim, so a
/// dependent job ran before the job it was waiting for. The claim now filters
/// on the dependency index: nothing is handed out while a dependency is
/// unfinished, and the dependent becomes claimable the moment it succeeds.
#[tokio::test]
async fn a_job_does_not_run_until_its_dependency_completes() {
    let queue = JobQueue::new_db(
        QueueConfig::default(),
        ferris_db::memory_db().await.unwrap(),
    );
    queue.init().await.unwrap();

    let dependency_id = queue
        .enqueue(Job::new("app.first".to_string(), ferris_core::Value::Null))
        .await
        .unwrap();
    let mut dependent = Job::new("app.second".to_string(), ferris_core::Value::Null);
    // Give the dependent the higher priority so only the dependency gate — not
    // priority order — can explain why it is not handed out first.
    dependent.priority = JobPriority::Critical;
    dependent.depends_on = vec![dependency_id.clone()];
    let dependent_id = queue.enqueue(dependent).await.unwrap();

    let first = queue
        .pop_job("worker-1", None)
        .await
        .unwrap()
        .expect("the dependency itself is claimable");
    assert_eq!(
        first.id, dependency_id,
        "the dependent job must wait for its dependency"
    );
    assert!(
        queue.pop_job("worker-1", None).await.unwrap().is_none(),
        "the dependent job is not claimable while its dependency is running"
    );

    queue
        .complete_job(&dependency_id, JobResult::success(ferris_core::Value::Null))
        .await
        .unwrap();

    let second = queue
        .pop_job("worker-1", None)
        .await
        .unwrap()
        .expect("the dependent job is claimable once its dependency succeeded");
    assert_eq!(second.id, dependent_id);
}

/// A dependency id that no longer exists (pruned history, a typo) must not
/// block a job forever: only a dependency row that still exists and has not
/// succeeded gates the claim.
#[tokio::test]
async fn a_dependency_that_no_longer_exists_does_not_block_the_claim() {
    let queue = JobQueue::new_db(
        QueueConfig::default(),
        ferris_db::memory_db().await.unwrap(),
    );
    queue.init().await.unwrap();

    let mut job = Job::new(
        "app.orphaned_dependency".to_string(),
        ferris_core::Value::Null,
    );
    job.depends_on = vec!["a-job-that-was-pruned".to_string()];
    queue.enqueue(job).await.unwrap();

    assert!(
        queue.pop_job("worker-1", None).await.unwrap().is_some(),
        "a missing dependency must not deadlock the queue"
    );
}
