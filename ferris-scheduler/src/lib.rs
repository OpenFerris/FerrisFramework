//! Ferris Scheduler - Cron-based job scheduler

mod leader;
pub mod persistence;
pub mod schedule;
pub mod scheduler;

pub use persistence::{ScheduleRecord, ScheduleStore};
pub use schedule::{
    CronExpression, Schedule, ScheduleConfig, ScheduleKind, ScheduledEvent, normalize_cron,
    parse_scheduler_events, parse_scheduler_events_json,
};
pub use scheduler::{Scheduler, SchedulerHandle};

// `redis` is a dev-dependency used by `tests/redis_e2e.rs`. The unit-test build
// links it without naming it, which trips `unused_crate_dependencies`.
#[cfg(test)]
use redis as _;
