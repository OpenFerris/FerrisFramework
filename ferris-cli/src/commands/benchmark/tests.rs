use super::concurrency::{CONCURRENCY_CEILING, DEFAULT_CONCURRENCY, clamp_concurrency, run_id};
use super::outcome::{Outcome, percentile};
use super::redis::{check_redis_is_writable, is_local_redis};
use std::time::Duration;

#[test]
fn percentile_clamps_bounds() {
    let empty: Vec<Duration> = vec![];
    assert_eq!(percentile(&empty, 95.0), Duration::ZERO);

    let mut samples = vec![Duration::from_millis(10), Duration::from_millis(20)];
    samples.sort();
    assert_eq!(percentile(&samples, 50.0), Duration::from_millis(10));
    assert_eq!(percentile(&samples, 100.0), Duration::from_millis(20));
}

#[test]
fn percentile_single_sample() {
    let samples = vec![Duration::from_millis(7)];
    for p in [0.0_f64, 50.0, 99.0, 100.0] {
        assert_eq!(percentile(&samples, p), Duration::from_millis(7));
    }
}

#[test]
fn a_failure_is_counted_and_not_measured() {
    let mut outcome = Outcome::default();
    outcome.record(Duration::from_millis(3), Ok::<(), String>(()));
    outcome.record(
        Duration::from_millis(900),
        Err::<(), _>("500 Internal Server Error".into()),
    );
    outcome.record(Duration::from_millis(4), Ok(()));

    assert_eq!(outcome.succeeded, 2, "only successes are ops");
    assert_eq!(outcome.errors, 1);
    assert_eq!(outcome.latencies.len(), 2, "a 500 must not become a sample");
    assert_eq!(
        outcome.first_error.as_deref(),
        Some("500 Internal Server Error")
    );
    // The mean is over successes, not over the total time including the 900 ms
    // spent waiting for the server to fail.
    outcome.report("API").unwrap();
}

#[test]
fn a_run_where_nothing_worked_is_an_error() {
    let mut outcome = Outcome::default();
    for _ in 0..3 {
        outcome.record(
            Duration::from_millis(500),
            Err::<(), _>("connection refused".into()),
        );
    }
    let err = outcome
        .report("API")
        .expect_err("three failures and no successes is not a measurement");
    let message = err.to_string();
    assert!(message.contains("nothing succeeded"), "{message}");
    assert!(message.contains("3 attempt(s) failed"), "{message}");
}

#[test]
fn concurrency_is_bounded_and_says_so() {
    assert_eq!(clamp_concurrency(1), (1, None));
    assert_eq!(
        clamp_concurrency(DEFAULT_CONCURRENCY).0,
        DEFAULT_CONCURRENCY
    );

    let (n, note) = clamp_concurrency(0);
    assert_eq!(n, 1);
    assert!(
        note.as_ref()
            .is_some_and(|note| note.contains("one worker")),
        "{note:?}"
    );

    let (n, note) = clamp_concurrency(100_000);
    assert_eq!(n, CONCURRENCY_CEILING);
    assert!(
        note.as_ref().is_some_and(|note| note.contains("capped")),
        "{note:?}"
    );
}

#[test]
fn writing_to_someone_elses_redis_needs_asking() {
    assert!(is_local_redis("redis://localhost:6379"));
    assert!(is_local_redis("redis://127.0.0.1:6379"));
    assert!(is_local_redis("redis://[::1]:6379"));
    assert!(is_local_redis("redis://localhost"));
    assert!(
        is_local_redis("redis://user:pass@127.0.0.1:6380"),
        "credentials do not make it remote"
    );

    assert!(!is_local_redis("redis://redis.internal:6379"));
    assert!(!is_local_redis("redis://prod.example.com:6379"));
    assert!(!is_local_redis("not a url at all"));

    let err = check_redis_is_writable(
        "redis://prod.example.com:6379",
        false,
        "jobs",
        "enqueues 1000 durable jobs",
    )
    .expect_err("a remote Redis is not this machine's to write to");
    let message = err.to_string();
    assert!(message.contains("--allow-remote-write"), "{message}");
    assert!(message.contains("prod.example.com:6379"), "{message}");
    assert!(
        message.contains("enqueues 1000 durable jobs"),
        "the error says what would be written there: {message}"
    );

    assert!(
        check_redis_is_writable(
            "redis://prod.example.com:6379",
            true,
            "jobs",
            "enqueues 1000 durable jobs"
        )
        .is_ok()
    );
    assert!(
        check_redis_is_writable(
            "redis://localhost:6379",
            false,
            "jobs",
            "publishes 1 message"
        )
        .is_ok()
    );
}

#[test]
fn two_runs_do_not_share_a_channel_or_a_queue() {
    assert_ne!(run_id(), run_id(), "the suffix must identify the run");
}
