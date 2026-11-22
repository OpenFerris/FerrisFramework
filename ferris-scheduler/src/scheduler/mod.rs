//! Scheduler configuration and main scheduler
//!
//! [`Scheduler`] owns the cron engine and the durable schedule table;
//! [`SchedulerHandle`] is the shared handle the server and the plugin host
//! hold.

mod core;
mod handle;

pub use core::Scheduler;
pub use handle::SchedulerHandle;
