//! Worker regressions: pool shutdown, slot accounting, orphan recovery and
//! panic supervision.

use super::*;
use crate::JobContext;
use crate::job::{Job, JobStatus};
use crate::queue::QueueConfig;
use async_trait::async_trait;

struct NoopHandler;

#[async_trait]
impl JobHandler for NoopHandler {
    type Payload = ferris_core::Value;
    type Output = ferris_core::Value;

    fn job_name(&self) -> &'static str {
        "noop"
    }

    async fn handle(&self, _payload: Self::Payload, _ctx: JobContext) -> Result<Self::Output> {
        Ok(ferris_core::Value::Null)
    }
}

async fn queue() -> Arc<JobQueue> {
    let queue = JobQueue::new_db(
        QueueConfig::default(),
        ferris_db::memory_db().await.expect("in-memory db"),
    );
    queue.init().await.expect("queue init");
    Arc::new(queue)
}

#[test]
fn the_orphan_window_is_three_heartbeats() {
    let three = i64::try_from(HEARTBEAT_INTERVAL_SECS * 3).expect("fits in i64");
    assert_eq!(ORPHAN_AFTER_SECS, three);
}

#[test]
fn test_worker_config() {
    let config = WorkerConfig::default();
    assert_eq!(config.concurrency, 10);
    assert_eq!(config.queues, vec!["default"]);
}

#[test]
fn test_worker_status() {
    let statuses = vec![
        WorkerStatus::Starting,
        WorkerStatus::Running,
        WorkerStatus::Stopping,
        WorkerStatus::Stopped,
    ];

    for status in statuses {
        let _ = format!("{:?}", status);
    }
}

/// JOB1: the pool's shutdown signal must reach the worker it spawned.
/// Before the fix `shutdown()` awaited a task parked on a private channel
/// that nothing could signal, so this timed out forever.
#[tokio::test]
async fn a_pool_shuts_down_its_worker() {
    let mut pool = WorkerPool::new();
    pool.add_worker(Worker::new(
        WorkerConfig {
            poll_interval_ms: 10,
            ..Default::default()
        },
        NoopHandler,
        queue().await,
    ));

    tokio::time::timeout(std::time::Duration::from_secs(5), pool.shutdown())
        .await
        .expect("pool shutdown must not hang");
}

/// The worker takes a slot *before* claiming a job.
///
/// The claim used to come first, so a job was marked `running` (with its
/// `started_at` stamped) while the task waited for a semaphore permit; the
/// job sat in that state without executing, and its recorded duration
/// included the wait. With no free slot a worker must leave the job
/// untouched.
#[tokio::test]
async fn a_worker_without_a_free_slot_does_not_claim_a_job() {
    let queue = queue().await;
    let job_id = queue
        .enqueue(Job::new("noop".to_string(), ferris_core::Value::Null))
        .await
        .expect("enqueue");

    let mut worker = Worker::new(
        WorkerConfig {
            concurrency: 1,
            poll_interval_ms: 10,
            ..Default::default()
        },
        NoopHandler,
        queue.clone(),
    );
    // Hold the worker's only permit, so claiming would have to wait for it.
    let held = worker
        .semaphore
        .clone()
        .acquire_owned()
        .await
        .expect("the worker has a free slot");

    let handle = tokio::spawn(async move { worker.start().await });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let waiting = queue
        .get_job(&job_id)
        .await
        .expect("read job")
        .expect("job");
    assert_eq!(
        waiting.status,
        JobStatus::Pending,
        "a job must not be marked running while no slot is free"
    );
    assert!(
        waiting.started_at.is_none(),
        "the claim (and its start stamp) must wait for the permit"
    );

    // Releasing the slot lets the worker claim and run it.
    drop(held);
    let mut completed = false;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let job = queue
            .get_job(&job_id)
            .await
            .expect("read job")
            .expect("job");
        if job.status == JobStatus::Success {
            completed = true;
            break;
        }
    }
    assert!(
        completed,
        "the worker must claim the job once a slot is free"
    );

    handle.abort();
}

struct PanicHandler;

#[async_trait]
impl JobHandler for PanicHandler {
    type Payload = ferris_core::Value;
    type Output = ferris_core::Value;

    fn job_name(&self) -> &'static str {
        "panic"
    }

    async fn handle(&self, _payload: Self::Payload, _ctx: JobContext) -> Result<Self::Output> {
        panic!("handler exploded")
    }
}

/// JOB-01: a job still `running` under a worker that stopped heartbeating
/// is reclaimed by the worker's own sweep.
#[tokio::test]
async fn a_worker_recovers_an_orphaned_running_job() {
    let queue = queue().await;
    let job_id = queue
        .enqueue(Job::new("noop".to_string(), ferris_core::Value::Null))
        .await
        .expect("enqueue");
    // Claim it as a worker that will never heartbeat again.
    let claimed = queue
        .pop_job("dead-worker", Some("default"))
        .await
        .expect("claim")
        .expect("a due job");
    assert_eq!(claimed.id, job_id);
    assert_eq!(claimed.status, JobStatus::Running);

    let worker = Worker::new(WorkerConfig::default(), NoopHandler, queue.clone());
    // A claim stamped in the current second is only "stale" for a negative
    // window; the periodic task uses three heartbeat intervals.
    let recovered = worker.recover_orphans(-1).await.expect("recovery");
    assert_eq!(recovered, 1);

    let job = queue.get_job(&job_id).await.expect("read").expect("job");
    assert_eq!(job.status, JobStatus::Retrying);
    assert!(job.worker_id.is_none(), "the dead worker no longer owns it");
}

/// JOB-02: a handler panic must fail the job and release the slot instead
/// of leaving the row `running` and `current_jobs` inflated.
#[tokio::test]
async fn a_panicking_handler_fails_the_job_and_releases_its_slot() {
    let queue = queue().await;
    let job_id = queue
        .enqueue(Job::new("panic".to_string(), ferris_core::Value::Null))
        .await
        .expect("enqueue");

    let mut worker = Worker::new(
        WorkerConfig {
            concurrency: 1,
            poll_interval_ms: 10,
            ..Default::default()
        },
        PanicHandler,
        queue.clone(),
    );
    let stats = worker.stats.clone();
    let handle = tokio::spawn(async move { worker.start().await });

    let mut settled = false;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let stats = stats.read();
        if stats.jobs_failed > 0 && stats.current_jobs == 0 {
            settled = true;
            break;
        }
    }
    assert!(
        settled,
        "the supervisor must fail the job and free the slot"
    );

    let job = queue.get_job(&job_id).await.expect("read").expect("job");
    assert_eq!(job.status, JobStatus::Retrying);
    let error = job
        .result
        .as_ref()
        .and_then(|result| result.error.as_deref())
        .unwrap_or_default();
    assert!(error.contains("panicked"), "{error}");

    handle.abort();
}
