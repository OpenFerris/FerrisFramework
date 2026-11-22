//! The background leadership loop.
//!
//! Renew while leader, retry acquisition while follower. One interval drives
//! both cadences: the tick is the follower's retry interval, and the heartbeat
//! only fires when it is due while leading. A transient database error keeps
//! leadership (the persisted deadline is the truth); only an explicit
//! `renew == false` — the row was taken over — demotes this instance.
//!
//! A takeover runs the catch-up **before** publishing leadership. The tick loop
//! only enqueues while the flag is set, so it cannot race the catch-up for the
//! same overdue schedule; the catch-up renews the lease per recovered run
//! instead (SCHED-04).

use super::LeaderDuties;
use crate::leader::LeaderLease;
use ferris_core::SchedulerConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{info, warn};

/// Timing derived from `SchedulerConfig`.
///
/// `tick_interval_secs` is the scheduler's cadence, so it sizes both the tick
/// loop and the lease: the TTL must outlive a few ticks, and the renewal
/// happens well inside it. The acquisition retry is deliberately shorter than
/// the TTL, so a follower notices a dead leader promptly without hammering the
/// database.
pub(super) struct RuntimeSettings {
    pub(super) tick: std::time::Duration,
    pub(super) heartbeat: std::time::Duration,
    pub(super) acquire_retry: std::time::Duration,
}

impl RuntimeSettings {
    pub(super) fn from_config(config: &SchedulerConfig) -> (Self, i64) {
        let tick_secs = config.tick_interval_secs.max(1);
        let ttl_secs = tick_secs.saturating_mul(2).max(30);
        let ttl = i64::try_from(ttl_secs).unwrap_or(i64::MAX);
        let heartbeat = std::time::Duration::from_secs((ttl_secs / 3).max(5));
        let acquire_retry = std::time::Duration::from_secs((tick_secs / 6).clamp(1, 5));
        (
            Self {
                tick: std::time::Duration::from_secs(tick_secs),
                heartbeat,
                acquire_retry,
            },
            ttl,
        )
    }
}

pub(super) async fn lease_loop(
    lease: Arc<LeaderLease>,
    is_leader: Arc<AtomicBool>,
    duties: Arc<LeaderDuties>,
    heartbeat: std::time::Duration,
    acquire_retry: std::time::Duration,
) {
    let mut interval = tokio::time::interval(acquire_retry);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_renewal = std::time::Instant::now()
        .checked_sub(heartbeat)
        .unwrap_or_else(std::time::Instant::now);

    loop {
        interval.tick().await;

        if is_leader.load(Ordering::SeqCst) {
            if last_renewal.elapsed() < heartbeat {
                continue;
            }
            match lease.renew().await {
                Ok(true) => {}
                Ok(false) => {
                    warn!(
                        "Scheduler instance {} lost the leader lease; stopping scheduled enqueues",
                        lease.owner()
                    );
                    is_leader.store(false, Ordering::SeqCst);
                }
                Err(e) => warn!("Scheduler lease renewal failed (will retry): {e}"),
            }
            last_renewal = std::time::Instant::now();
        } else {
            match lease.try_acquire().await {
                Ok(true) => {
                    info!(
                        "Scheduler instance {} acquired the leader lease",
                        lease.owner()
                    );
                    // Catch up while still unpublished: the tick loop must not
                    // see leadership until the overdue runs are recovered, or
                    // both paths would enqueue the same schedule (SCHED-04).
                    if duties.on_became_leader().await {
                        is_leader.store(true, Ordering::SeqCst);
                    } else {
                        warn!(
                            "Scheduler instance {} lost the lease during the takeover catch-up",
                            lease.owner()
                        );
                    }
                    last_renewal = std::time::Instant::now();
                }
                Ok(false) => {}
                Err(e) => warn!("Scheduler lease acquisition failed (will retry): {e}"),
            }
        }
    }
}
