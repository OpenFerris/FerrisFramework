//! Ferris Jobs - Background job queue with Redis + SQL persistence

pub mod dispatcher;
pub mod job;
pub mod ports;
pub mod queue;
pub mod retry;
pub mod store;
pub mod worker;

pub use dispatcher::JobDispatcher;
pub use job::{Job, JobContext, JobHandler, JobPriority, JobResult, JobStatus};
pub use queue::{JobQueue, QueueConfig, QueueStats};
pub use retry::{ExponentialBackoff, RetryPolicy};
pub use store::{JobStore, JobWorker};
pub use worker::{Worker, WorkerConfig, WorkerPool};

// `ferris-macro` is a dev-dependency used by the `#[job]` integration test. The
// unit-test build links every dev-dependency but never names this one, which
// trips `unused_crate_dependencies`; anchoring it here keeps the lint honest for
// the real dependencies (JOB15).
#[cfg(test)]
use ferris_macro as _;
