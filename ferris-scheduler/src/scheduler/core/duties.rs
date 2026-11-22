//! The work only the leader may do: the boot/takeover catch-up and the history
//! purge.
//!
//! Kept as its own type so the background lease loop can run it without
//! holding an `Arc<Scheduler>` (which would make the loop recursive with the
//! tick loop it starts).
//!
//! # Fencing
//!
//! A catch-up with a large backlog can outlive the lease TTL. The old code
//! enqueued every recovered run after a single acquisition, so a successor that
//! took over mid-backlog received a second copy of each remaining schedule
//! (SCHED-04). Every recovered run now renews the lease first; `renew == false`
//! (or a renewal error, which cannot prove ownership) aborts the catch-up and
//! reports that leadership was lost, so the caller does not publish it.

use super::{MISFIRE_GRACE_SECS, build_and_enqueue, refresh_schedule};
use crate::leader::LeaderLease;
use crate::persistence::ScheduleStore;
use crate::schedule::Schedule;
use chrono::Utc;
use chrono_tz::Tz;
use ferris_jobs::JobQueue;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

pub(super) struct LeaderDuties {
    store: Arc<ScheduleStore>,
    job_queue: Arc<JobQueue>,
    schedules: Arc<RwLock<HashMap<String, Schedule>>>,
    history_retention_days: u32,
    lease: Arc<LeaderLease>,
    timezone: Tz,
}

impl LeaderDuties {
    pub(super) const fn new(
        store: Arc<ScheduleStore>,
        job_queue: Arc<JobQueue>,
        schedules: Arc<RwLock<HashMap<String, Schedule>>>,
        history_retention_days: u32,
        lease: Arc<LeaderLease>,
        timezone: Tz,
    ) -> Self {
        Self {
            store,
            job_queue,
            schedules,
            history_retention_days,
            lease,
            timezone,
        }
    }

    /// Renew the lease as the fence for one step of the catch-up.
    ///
    /// Returns `false` when the row belongs to another owner (a takeover) or
    /// when the renewal cannot be proven, in which case no further run may be
    /// enqueued.
    async fn renew_lease(&self, step: &str) -> bool {
        match self.lease.renew().await {
            Ok(true) => true,
            Ok(false) => {
                warn!(
                    "Aborting the missed-run catch-up: {} no longer holds the lease ({step})",
                    self.lease.owner()
                );
                false
            }
            Err(e) => {
                warn!("Aborting the missed-run catch-up: cannot renew the lease ({step}): {e}");
                false
            }
        }
    }

    /// Enqueue every schedule whose persisted deadline was missed, once each.
    ///
    /// A12: executions that fired while no leader was listening used to be
    /// dropped silently. The persisted `next_run` is the promise; when it lies
    /// in the past by more than the misfire grace, the run is recovered here
    /// (coalesced to a single execution) and the deadline is rolled forward so
    /// it cannot fire again at the next restart or takeover.
    ///
    /// Returns `false` when the lease is no longer provably ours, so the caller
    /// keeps this instance a follower.
    async fn catch_up_missed_runs(&self) -> bool {
        let now = Utc::now();

        // Fencing starts before the backlog is read: an instance that was
        // already replaced must report that even when nothing is missed.
        if !self.renew_lease("before the missed-run catch-up").await {
            return false;
        }

        let records = match self.store.list().await {
            Ok(records) => records,
            Err(e) => {
                error!("Failed to load schedules for the missed-run catch-up: {e}");
                return true;
            }
        };

        for schedule in records {
            if !schedule.missed_while_down(now, MISFIRE_GRACE_SECS) {
                continue;
            }

            // Fencing: prove the lease immediately before every enqueue. A
            // successor that took over changed `owner`, so renewal fails and
            // this instance stops instead of duplicating the backlog.
            if !self.renew_lease("before recovering a missed run").await {
                return false;
            }

            let result = build_and_enqueue(&self.job_queue, &schedule.config).await;
            match &result {
                Ok(job_id) => info!(
                    "Recovered missed schedule run {} for job {} → {job_id}",
                    schedule.id, schedule.config.job_name
                ),
                Err(e) => error!(
                    "Failed to recover missed schedule run {}: {}",
                    schedule.id, e
                ),
            }

            let next = schedule.calculate_next_run_in(self.timezone, now);
            if let Err(e) = self
                .store
                .update_run_info(
                    &schedule.id,
                    now,
                    next,
                    result.is_ok(),
                    result.as_ref().err().map(ToString::to_string),
                )
                .await
            {
                error!(
                    "Failed to record the recovered run of schedule {}: {e}",
                    schedule.id
                );
            }
            refresh_schedule(&self.store, &self.schedules, &schedule.id).await;
        }

        true
    }

    /// Purge terminal job history older than `history_retention_days`.
    ///
    /// The `schedules` table stores only the live definition (counters, last
    /// error) — there is no per-run history table to prune — so the retention
    /// window applies to the `jobs` rows the scheduled runs produced, which is
    /// where the run history actually lives. Only terminal rows are deleted.
    async fn purge_history(&self) {
        let days = self.history_retention_days;
        if days == 0 {
            return;
        }
        let cutoff = Utc::now() - chrono::Duration::days(i64::from(days));
        match self.job_queue.purge_terminal_jobs(cutoff).await {
            Ok(0) => {}
            Ok(purged) => info!("Purged {purged} terminal job rows older than {days} days"),
            Err(e) => warn!("Failed to purge job history older than {days} days: {e}"),
        }
    }

    /// Run the takeover duties. Returns whether this instance still holds the
    /// lease and may be published as leader.
    pub(super) async fn on_became_leader(&self) -> bool {
        info!("Scheduler became the leader; running the missed-run catch-up");
        let still_leading = self.catch_up_missed_runs().await;
        if still_leading {
            self.purge_history().await;
        }
        still_leading
    }
}
