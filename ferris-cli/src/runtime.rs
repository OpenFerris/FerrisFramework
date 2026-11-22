//! The tokio runtime a command's blocking work runs on.
//!
//! Six commands each built their own, and they disagreed: `migrate` and
//! `doctor` asked for a current-thread runtime, `tenant` asked for a multi-thread
//! one with a single worker (which is a current-thread runtime with extra
//! bookkeeping), and the three benchmarks asked for a full multi-thread one
//! whether or not they spawn anything. One place decides.

use anyhow::Result;

/// A runtime for `block_on` work in a synchronous command.
///
/// `workers` is `None` for a current-thread runtime — enough for a migration or a
/// health check — and `Some(n)` when the command really spawns `n` tasks, which
/// is what the database benchmark does.
pub fn block_on_runtime(workers: Option<usize>) -> Result<tokio::runtime::Runtime> {
    let mut builder = match workers {
        Some(_) => tokio::runtime::Builder::new_multi_thread(),
        None => tokio::runtime::Builder::new_current_thread(),
    };
    if let Some(workers) = workers {
        builder.worker_threads(workers.max(1));
    }
    builder
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to start async runtime: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_shapes_build_and_run() {
        for workers in [None, Some(1), Some(4)] {
            let rt = block_on_runtime(workers)
                .unwrap_or_else(|e| panic!("{workers:?} must build a runtime: {e}"));
            // A runtime that cannot drive the reactor is not a runtime.
            let answer = rt.block_on(async { 1 + 1 });
            assert_eq!(answer, 2, "{workers:?}");
        }
    }

    #[test]
    fn a_worker_count_of_zero_still_gives_a_worker() {
        // `worker_threads(0)` is a panic inside tokio, and `clamp_concurrency`
        // is not the only thing that can produce a zero.
        let rt = block_on_runtime(Some(0)).expect("zero workers must not panic");
        assert_eq!(rt.block_on(async { 7 }), 7);
    }
}
