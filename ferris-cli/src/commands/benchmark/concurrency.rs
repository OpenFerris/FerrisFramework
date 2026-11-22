//! Worker-count policy and the per-run identifier.

use std::time::{SystemTime, UNIX_EPOCH};

/// Upper bound on `--concurrency`.
///
/// The number drives both OS threads and database connections, so it is the
/// user's dial to turn up, not something to let a typo ask for 100k of.
pub(super) const CONCURRENCY_CEILING: usize = 256;

/// `SELECT 1` against a pool, not a load test: the default was 100 workers,
/// which is 100 threads and 100 connections to answer "is the database there".
pub const DEFAULT_CONCURRENCY: usize = 8;

/// The concurrency to actually use, with a word about it when it is not what was
/// asked for.
pub(super) fn clamp_concurrency(requested: usize) -> (usize, Option<String>) {
    match requested {
        0 => (1, Some("--concurrency 0 means one worker".to_string())),
        n if n > CONCURRENCY_CEILING => (
            CONCURRENCY_CEILING,
            Some(format!(
                "--concurrency {n} capped at {CONCURRENCY_CEILING} worker threads \
                 and database connections"
            )),
        ),
        n => (n, None),
    }
}

/// A unique suffix, so two runs (or a run and the developer's own queues) never
/// share a channel or a queue name.
pub(super) fn run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{}-{}", std::process::id(), nanos)
}
