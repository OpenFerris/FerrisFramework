//! The enqueueing tick loop.
//!
//! One loop per scheduler instance wakes every `tick_interval_secs` (or as soon
//! as a schedule changes) and enqueues every enabled schedule whose persisted
//! deadline has passed. It replaces the per-schedule `tokio-cron-scheduler`
//! closures because that engine freezes the zone offset when a job is
//! registered, so a DST transition left cron firing at the old offset
//! (SCHED-02). Resolving the deadline here, in the configured `chrono-tz` zone,
//! keeps the wall-clock promise and persists the UTC instant.
//!
//! Only the leader enqueues: every instance runs the loop, and without the
//! lease check N instances would enqueue N copies of each run. Leadership is
//! published only after the takeover catch-up, so the two paths cannot
//! double-enqueue the same overdue schedule (SCHED-04).

use super::{build_and_enqueue, refresh_schedule};
use crate::persistence::ScheduleStore;
use crate::schedule::Schedule;
use chrono::Utc;
use chrono_tz::Tz;
use ferris_jobs::JobQueue;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Notify, RwLock, Semaphore};
use tracing::{error, info};

/// Everything the loop needs, cloned out of the scheduler so the task owns no
/// reference to the `Arc<Scheduler>` it belongs to.
pub(super) struct TickContext {
    pub(super) store: Arc<ScheduleStore>,
    pub(super) job_queue: Arc<JobQueue>,
    pub(super) schedules: Arc<RwLock<HashMap<String, Schedule>>>,
    pub(super) is_leader: Arc<AtomicBool>,
    pub(super) slots: Arc<Semaphore>,
    pub(super) timezone: Tz,
}

pub(super) async fn tick_loop(context: TickContext, tick: std::time::Duration, wake: Arc<Notify>) {
    let mut ticker = tokio::time::interval(tick.max(std::time::Duration::from_secs(1)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = wake.notified() => {}
        }

        if !context.is_leader.load(Ordering::SeqCst) {
            continue;
        }

        let now = Utc::now();
        let due = due_schedules(&context.schedules, now).await;

        for schedule in due {
            // Leadership can lapse between two entries of a burst.
            if !context.is_leader.load(Ordering::SeqCst) {
                break;
            }

            // A disable/remove can race the snapshot; re-read before spending
            // a permit or an enqueue on it.
            let still_due = context
                .schedules
                .read()
                .await
                .get(&schedule.id)
                .is_some_and(|live| live.enabled && live.next_run.is_some_and(|next| next <= now));
            if !still_due {
                continue;
            }

            // Bound the number of scheduled enqueues in flight at once.
            let permit = match context.slots.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(e) => {
                    error!("Scheduler enqueue slot closed for {}: {e}", schedule.id);
                    return;
                }
            };

            // Advance from `now`, not from the persisted (possibly much older)
            // deadline: a long downtime coalesces into one run instead of
            // replaying every missed tick.
            let next = schedule.calculate_next_run_in(context.timezone, now);

            match build_and_enqueue(&context.job_queue, &schedule.config).await {
                Ok(job_id) => {
                    info!("Triggered scheduled job: {} ({})", schedule.id, job_id);
                    if let Err(e) = context
                        .store
                        .update_run_info(&schedule.id, now, next, true, None)
                        .await
                    {
                        error!("Failed to record run of schedule {}: {e}", schedule.id);
                    }
                }
                Err(e) => {
                    error!("Failed to enqueue scheduled job {}: {}", schedule.id, e);
                    if let Err(e) = context
                        .store
                        .update_run_info(&schedule.id, now, next, false, Some(e.to_string()))
                        .await
                    {
                        error!("Failed to record run of schedule {}: {e}", schedule.id);
                    }
                }
            }
            drop(permit);

            // Mirror the whole record, not just the deadline, so the read
            // model reports the counters and the last error (SCH3).
            refresh_schedule(&context.store, &context.schedules, &schedule.id).await;
        }
    }
}

/// Snapshot the enabled schedules whose persisted deadline is due.
async fn due_schedules(
    schedules: &RwLock<HashMap<String, Schedule>>,
    now: chrono::DateTime<Utc>,
) -> Vec<Schedule> {
    schedules
        .read()
        .await
        .values()
        .filter(|schedule| schedule.enabled && schedule.next_run.is_some_and(|next| next <= now))
        .cloned()
        .collect()
}
