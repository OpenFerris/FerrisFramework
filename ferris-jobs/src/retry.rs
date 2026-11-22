//! Retry backoff for failed jobs.
//!
//! One policy, because the queue has one: `fail_job` asks `next_delay` how long
//! to wait before the next attempt and writes that deadline to the job. The
//! fixed/linear policies and the builder that could produce them were never
//! reachable (`JobQueue::with_retry_policy` had no caller), and
//! `should_retry`/`max_retries` duplicated the cap that `Job::can_retry`
//! already enforces from the job's own `max_retries` (JOB7).

use std::time::Duration;

/// Retry policy trait
pub trait RetryPolicy: Send + Sync {
    /// Calculate delay before next retry
    fn next_delay(&self, attempt: u32) -> Duration;
}

/// Exponential backoff retry policy
#[derive(Debug, Clone)]
pub struct ExponentialBackoff {
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
    pub jitter: bool,
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self {
            base_delay: Duration::from_secs(60),
            max_delay: Duration::from_secs(3600), // 1 hour
            multiplier: 2.0,
            jitter: true,
        }
    }
}

impl RetryPolicy for ExponentialBackoff {
    fn next_delay(&self, attempt: u32) -> Duration {
        let delay = self.base_delay.as_secs_f64() * self.multiplier.powi(attempt as i32);
        let delay = Duration::from_secs_f64(delay.min(self.max_delay.as_secs_f64()));

        if self.jitter {
            // Add ±25% jitter
            let jitter_range = delay.as_secs_f64() * 0.25;
            let jitter = (rand::random::<f64>() - 0.5) * 2.0 * jitter_range;
            Duration::from_secs_f64((delay.as_secs_f64() + jitter).max(0.0))
        } else {
            delay
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exponential_backoff() {
        let policy = ExponentialBackoff::default();

        let delay1 = policy.next_delay(0);
        let delay2 = policy.next_delay(1);
        let delay3 = policy.next_delay(2);

        // Should increase exponentially
        assert!(delay2 >= delay1);
        assert!(delay3 >= delay2);
    }

    #[test]
    fn the_delay_is_capped_at_max_delay() {
        let policy = ExponentialBackoff {
            jitter: false,
            base_delay: Duration::from_secs(60),
            max_delay: Duration::from_secs(300),
            multiplier: 2.0,
        };
        assert_eq!(policy.next_delay(10), Duration::from_secs(300));
    }
}
