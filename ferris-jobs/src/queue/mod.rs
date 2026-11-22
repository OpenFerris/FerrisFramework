//! The job queue: the durable store plus the Redis sorted-set gauge.
//!
//! Split into the queue itself ([`core`]) and its tests so one file holds the
//! implementation and another its suite. `core` re-exports through this
//! module, so every `crate::queue::JobQueue` path keeps resolving.

mod core;

pub use core::{JobQueue, QueueConfig, QueueStats};
