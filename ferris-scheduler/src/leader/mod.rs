//! Multi-instance leadership for the scheduler.
//!
//! Every process running the scheduler role loads the same `schedules` table
//! and registers the same cron jobs. Without coordination each process fires
//! every schedule, so N instances enqueue N copies of each run per tick. The
//! fix is a lease in the database: only the holder of a non-expired
//! `scheduler_lease` row enqueues (and performs the boot catch-up); the others
//! keep their engine running but their cron closures are inert until they win
//! the lease.
//!
//! The lease is deliberately database-only. Redis is optional in this
//! framework (`JobQueue::new_db`), and a scheduler that needs Redis to decide
//! who is in charge would not work on the SQLite edge deployment.

mod lease;

pub use lease::LeaderLease;

#[cfg(test)]
mod tests;
