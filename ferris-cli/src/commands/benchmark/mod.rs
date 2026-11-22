//! Benchmark command
//!
//! Real benchmarks, no placeholders:
//! - `api`      -> HTTP round-trips against a running server (`/api/health`)
//! - `database` -> `SELECT 1` round-trips through the configured pool
//! - `realtime` -> Redis pub/sub publish throughput
//! - `jobs`     -> job enqueue throughput through `ferris_jobs::JobQueue`
//! - `print`    -> Typst PDF render of the built-in invoice template
//!
//! Three things this file used to get wrong, and they all had the same shape —
//! a measurement that looked fine because nothing checked it:
//!
//! 1. A failed operation still pushed a latency sample. Only successes counted
//!    as `ops`, so a server answering 500 to everything printed `ops: 0`,
//!    `throughput 0.0` and a healthy-looking p50 measured from the failures. An
//!    `Outcome` now records only successes and reports the errors, and a run
//!    that measured nothing is an error rather than a pass.
//! 2. `--concurrency` defaulted to 100, and a `SELECT 1` benchmark took it as
//!    100 OS threads and 100 database connections.
//! 3. The `realtime` and `jobs` benchmarks *wrote* to the configured Redis —
//!    `iterations` published messages and `iterations` durable jobs — with no
//!    warning and nothing to clean up. They now name a channel and a queue of
//!    their own, say what they are about to write, delete the queue afterwards,
//!    and need `--allow-remote-write` for a Redis that is not this machine.

mod concurrency;
mod outcome;
mod redis;
mod run;
mod scenarios;

pub use concurrency::DEFAULT_CONCURRENCY;
pub use run::run;

#[cfg(test)]
mod tests;
