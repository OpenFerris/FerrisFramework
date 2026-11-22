//! The job-intake loop.
//!
//! One tick takes a semaphore slot **before** claiming (a claimed but parked
//! job looks like an orphan to every other worker), pops at most one job, runs
//! it under a child task that owns the permit, and supervises that task so a
//! handler panic becomes a failed job instead of a slot that never returns.

use crate::{JobContext, JobHandler, JobResult};
use ferris_core::{Error, JobError};
use tokio::time::{Duration, interval};
use tracing::error;

use super::{JobSlot, Worker, WorkerStatus};

impl<H: JobHandler + 'static> Worker<H> {
    /// Start job processing loop
    pub(super) fn start_processing(&self) -> tokio::task::JoinHandle<()> {
        let handler = self.handler.clone();
        let queue = self.queue.clone();
        let config = self.config.clone();
        let semaphore = self.semaphore.clone();
        let status = self.status.clone();
        let stats = self.stats.clone();
        let mut shutdown_rx = self.shutdown_rx.clone();
        let worker_id = self.config.worker_id.clone();

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_millis(config.poll_interval_ms));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if *shutdown_rx.borrow() { break; }
                        if *status.read() != WorkerStatus::Running { continue; }

                        // Take a slot *before* claiming. The claim used to come
                        // first, so a job was marked `running` — and its
                        // `started_at` stamped — while the task was still
                        // waiting for a semaphore permit. Instead of running,
                        // the job sat in a state another process would treat as
                        // an orphan. `try_acquire` never waits: a full worker
                        // simply polls again.
                        let permit = match semaphore.clone().try_acquire_owned() {
                            Ok(permit) => permit,
                            Err(tokio::sync::TryAcquireError::NoPermits) => continue,
                            Err(tokio::sync::TryAcquireError::Closed) => {
                                // A closed semaphore is a programming error, not
                                // a reason to panic the worker task (JOB12).
                                error!("Worker {worker_id} semaphore closed; stopping job intake");
                                break;
                            }
                        };

                        // Try to pop job from queues, at most one per tick so a
                        // slot is never held for an empty queue.
                        let mut claimed = None;
                        for queue_name in &config.queues {
                            if *shutdown_rx.borrow() { break; }
                            match queue.pop_job(&worker_id, Some(queue_name)).await {
                                Ok(Some(job)) => {
                                    claimed = Some(job);
                                    break;
                                }
                                Ok(None) => {
                                    // No job available, try next queue
                                }
                                Err(e) => {
                                    error!("Error popping job from queue {}: {}", queue_name, e);
                                }
                            }
                        }
                        // Nothing to run: releasing the permit here is what
                        // makes the next tick retry.
                        let Some(job) = claimed else { continue };

                        let queue = queue.clone();

                        // Update stats
                        stats.write().current_jobs += 1;

                        let handler = handler.clone();
                        let stats = stats.clone();
                        let job_id = job.id.clone();
                        let supervisor_queue = queue.clone();
                        let supervisor_stats = stats.clone();

                        // The run task owns the permit; the supervisor owns the
                        // slot and turns a panic into a failed job (JOB-02).
                        let run = tokio::spawn(async move {
                            let _permit = permit; // Hold permit until job completes

                                        let start = std::time::Instant::now();
                                        let context = JobContext::new(job.id.clone(), job.name.clone(), job.queue.clone())
                                            .with_attempt(job.retries + 1);

                                        let payload_value: ferris_core::Value = job.payload.clone();
                                        let json_value = match serde_json::to_value(&payload_value) {
                                            Ok(value) => value,
                                            Err(e) => {
                                                error!("Failed to encode job payload: {}", e);
                                                if let Err(e) = queue
                                                    .fail_job(
                                                        &job.id,
                                                        format!("Payload encoding error: {e}"),
                                                    )
                                                    .await
                                                {
                                                    error!("Failed to fail job {}: {}", job.id, e);
                                                }
                                                stats.write().jobs_failed += 1;
                                                return;
                                            }
                                        };
                                        let payload: std::result::Result<H::Payload, _> = serde_json::from_value(json_value);
                                        let payload = match payload {
                                            Ok(p) => p,
                                            Err(e) => {
                                                error!("Failed to deserialize job payload: {}", e);
                                                if let Err(e) = queue.fail_job(&job.id, format!("Deserialization error: {e}")).await {
                                                    error!("Failed to fail job {}: {}", job.id, e);
                                                }
                                                stats.write().jobs_failed += 1;
                                                return;
                                            }
                                        };

                                        // Enforce the job timeout; a hung handler must not
                                        // occupy a worker slot forever.
                                        let result = match tokio::time::timeout(
                                            job.timeout,
                                            handler.handle(payload, context),
                                        )
                                        .await
                                        {
                                            Ok(result) => result,
                                            Err(_) => Err(Error::Job(JobError::Timeout)),
                                        };

                                        match result {
                                            Ok(output) => {
                                                let duration = start.elapsed().as_millis() as u64;
                                                match serde_json::to_value(&output) {
                                                    Ok(json_value) => {
                                                        let output_value: ferris_core::Value = ferris_core::Value::from_json(json_value);
                                                        let result = JobResult {
                                                            success: true,
                                                            output: Some(output_value),
                                                            error: None,
                                                            started_at: chrono::Utc::now() - chrono::Duration::milliseconds(duration as i64),
                                                            completed_at: chrono::Utc::now(),
                                                            duration_ms: duration,
                                                        };
                                                        if let Err(e) = queue.complete_job(&job.id, result).await {
                                                            error!("Failed to complete job {}: {}", job.id, e);
                                                        }
                                                        stats.write().jobs_processed += 1;
                                                    }
                                                    Err(e) => {
                                                        // An output that cannot be
                                                        // encoded is a failed job, not
                                                        // a `null` result (JOB10).
                                                        if let Err(e) = queue.fail_job(&job.id, format!("Output encoding error: {e}")).await {
                                                            error!("Failed to fail job {}: {}", job.id, e);
                                                        }
                                                        stats.write().jobs_failed += 1;
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                let error = e.to_string();
                                                if let Err(e) = queue.fail_job(&job.id, error).await {
                                                    error!("Failed to fail job {}: {}", job.id, e);
                                                }
                                                stats.write().jobs_failed += 1;
                                            }
                                        }
                            // `_permit` drops here; `JobSlot` releases the
                            // count once the supervisor observes this task.
                        });

                        tokio::spawn(async move {
                            let _slot = JobSlot::new(supervisor_stats.clone());
                            if let Err(join_error) = run.await {
                                error!(
                                    "Job {job_id} terminated without completing: {join_error}"
                                );
                                if let Err(e) = supervisor_queue
                                    .fail_job(&job_id, format!("Job task failed: {join_error}"))
                                    .await
                                {
                                    error!("Failed to fail job {job_id}: {e}");
                                }
                                supervisor_stats.write().jobs_failed += 1;
                            }
                        });
                    }
                    _ = shutdown_rx.changed() => {
                        break;
                    }
                }
            }
        })
    }
}
