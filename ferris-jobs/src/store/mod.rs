//! SQL persistence for the job queue.
//!
//! Kept apart from [`crate::queue::JobQueue`] so the durable half of the queue
//! can be exercised without a Redis connection, and so every statement lives in
//! one place instead of being spread through the Redis-facing methods.
//!
//! Two shapes are stored per job: `payload` holds the serialised [`Job`] and is
//! what is read back, while the columns beside it exist so the queue can be
//! claimed and counted in SQL without decoding every row.

mod columns;
mod jobs;
mod schema;
mod workers;

pub use jobs::{JobStore, RecoveredJob};
pub use schema::{TAB_JOB_DEPENDENCIES, TAB_JOB_WORKERS, TAB_JOBS};
pub use workers::JobWorker;

#[cfg(test)]
mod tests;
