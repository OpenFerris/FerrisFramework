//! What a benchmark run observed, and how it is reported.

use anyhow::{Result, anyhow};
use std::time::Duration;

/// What a benchmark run observed.
///
/// Failures are not samples. Recording the elapsed time of a 500, a refused
/// connection or a render error and then dividing the sum by the number of
/// *successes* is how a run with nothing working prints a plausible p50.
#[derive(Debug, Default)]
pub(super) struct Outcome {
    pub(super) latencies: Vec<Duration>,
    pub(super) succeeded: usize,
    pub(super) errors: usize,
    pub(super) first_error: Option<String>,
}

impl Outcome {
    /// Time one operation. `Ok` is a sample; `Err` is a counted failure with a
    /// message to show.
    pub(super) fn record<T>(&mut self, elapsed: Duration, result: Result<T, String>) {
        match result {
            Ok(_) => {
                self.succeeded += 1;
                self.latencies.push(elapsed);
            }
            Err(message) => {
                self.errors += 1;
                if self.first_error.is_none() {
                    self.first_error = Some(message);
                }
            }
        }
    }

    /// Print the report. A run where nothing succeeded is an error: it measured
    /// nothing, and reporting `ops: 0` as a result is how this hid for so long.
    pub(super) fn report(self, title: &str) -> Result<()> {
        let Self {
            mut latencies,
            succeeded,
            errors,
            first_error,
        } = self;
        latencies.sort();

        let total: Duration = latencies.iter().copied().sum();
        let mean = if succeeded == 0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(total.as_secs_f64() / succeeded as f64)
        };
        let throughput = if total.is_zero() {
            0.0
        } else {
            succeeded as f64 / total.as_secs_f64()
        };

        println!();
        println!("== {title} ==");
        println!("  ops:           {succeeded}");
        println!("  throughput:    {throughput:.1} ops/s");
        println!(
            "  min:           {:.2}ms",
            percentile(&latencies, 0.0).as_secs_f64() * 1000.0
        );
        println!("  mean:          {:.2}ms", mean.as_secs_f64() * 1000.0);
        println!(
            "  p50:           {:.2}ms",
            percentile(&latencies, 50.0).as_secs_f64() * 1000.0
        );
        println!(
            "  p95:           {:.2}ms",
            percentile(&latencies, 95.0).as_secs_f64() * 1000.0
        );
        println!(
            "  p99:           {:.2}ms",
            percentile(&latencies, 99.0).as_secs_f64() * 1000.0
        );
        println!(
            "  max:           {:.2}ms",
            percentile(&latencies, 100.0).as_secs_f64() * 1000.0
        );
        if errors > 0 {
            println!("  errors:        {errors}");
            if let Some(message) = &first_error {
                println!("  first error:   {message}");
            }
        }

        if succeeded == 0 {
            return Err(anyhow!(
                "{title}: nothing succeeded ({} attempt(s) failed) — the numbers above \
                 measure no work at all",
                errors
            ));
        }
        Ok(())
    }
}

/// Bounded percentile lookup. `sorted` must be pre-sorted ascending; empty
/// samples return `Duration::ZERO`. The rank is clamped into `[0, len-1]`.
pub(super) fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = (sorted.len() as f64 * p / 100.0).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}
