//! Schedule definitions
//!
//! The cron expression is the only trigger shape the crate persists, so the
//! model carries exactly that: no `Interval` alternative that the engine would
//! ignore, and no second, hand-rolled cron parser whose field mapping could
//! disagree with the engine's (SCH4/SCH8). `next_occurrence` parses with the
//! same `croner` version `tokio-cron-scheduler` validates with (SCH10).
//!
//! # Timezone
//!
//! A cron expression is a local-time promise ("daily at 09:00"), while every
//! stored instant is UTC. [`CronExpression::next_occurrence_in`] resolves the
//! promise in the configured IANA zone and returns the UTC instant, so a DST
//! transition keeps the wall-clock hour instead of silently shifting the run
//! (SCHED-02). The UTC form of the same parser is kept for callers that only
//! want the framework default.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use ferris_core::{Error, Result, ValidationError, Value};
use ferris_jobs::JobPriority;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Schedule configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleConfig {
    pub cron_expression: String,
    pub job_name: String,
    pub payload: Value,
    pub queue: String,
    pub priority: JobPriority,
    pub timeout: std::time::Duration,
    pub max_retries: u32,
    pub tags: Vec<String>,
    pub metadata: std::collections::HashMap<String, Value>,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            cron_expression: "0 0 * * *".to_string(), // Daily at midnight
            job_name: "Scheduled Job".to_string(),
            payload: Value::Null,
            queue: "default".to_string(),
            priority: JobPriority::Normal,
            timeout: std::time::Duration::from_secs(300),
            max_retries: 3,
            tags: Vec::new(),
            metadata: std::collections::HashMap::new(),
        }
    }
}

/// Schedule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub id: String,
    pub name: String,
    pub config: ScheduleConfig,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_run: Option<DateTime<Utc>>,
    pub next_run: Option<DateTime<Utc>>,
    pub run_count: u64,
    pub failure_count: u64,
    pub last_error: Option<String>,
}

impl Schedule {
    pub fn new(name: String, config: ScheduleConfig) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            config,
            enabled: true,
            created_at: now,
            updated_at: now,
            last_run: None,
            next_run: None,
            run_count: 0,
            failure_count: 0,
            last_error: None,
        }
    }

    pub fn with_cron(mut self, cron: String) -> Self {
        self.config.cron_expression = cron;
        self
    }

    pub fn with_job_name(mut self, name: String) -> Self {
        self.config.job_name = name;
        self
    }

    pub fn with_payload(mut self, payload: Value) -> Self {
        self.config.payload = payload;
        self
    }

    pub fn with_queue(mut self, queue: String) -> Self {
        self.config.queue = queue;
        self
    }

    pub fn with_priority(mut self, priority: JobPriority) -> Self {
        self.config.priority = priority;
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.config.tags = tags;
        self
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Calculate the next run time from the cron expression in UTC.
    pub fn calculate_next_run(&self, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        CronExpression::next_occurrence(&self.config.cron_expression, from)
    }

    /// Calculate the next run time from the cron expression in `timezone`.
    ///
    /// This is what the scheduler uses: the expression means a wall-clock
    /// moment in the configured zone, and the returned instant is UTC so it is
    /// persisted unchanged (SCHED-02).
    #[must_use]
    pub fn calculate_next_run_in(
        &self,
        timezone: Tz,
        from: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        CronExpression::next_occurrence_in(&self.config.cron_expression, timezone, from)
    }

    /// Whether an execution was missed while the process was down.
    ///
    /// The persisted `next_run` is the deadline the process promised to meet;
    /// when it lies in the past by more than the misfire grace, the run
    /// happened while nobody was listening and should be recovered on boot —
    /// coalesced to a single run, not one per missed tick. `None` (a deadline
    /// never computed yet) is never "missed".
    #[must_use]
    pub fn missed_while_down(&self, now: DateTime<Utc>, grace_secs: i64) -> bool {
        self.next_run
            .is_some_and(|due| self.enabled && (now - due).num_seconds() >= grace_secs)
    }

    /// Record a run
    pub fn record_run(&mut self, success: bool, error: Option<String>) {
        self.last_run = Some(Utc::now());
        self.run_count += 1;

        if success {
            self.failure_count = 0;
            self.last_error = None;
        } else {
            self.failure_count += 1;
            self.last_error = error;
        }

        self.updated_at = Utc::now();
    }
}

/// The cadences a Frappe `scheduler_events` declaration can name.
///
/// Each fixed cadence expands to the cron expression Frappe uses, plus an
/// explicit [`ScheduleKind::Cron`] for anything else. `All` is the scheduler's
/// own tick: it means "at startup and on every scheduler interval", which is
/// expressed as an every-second cron and bounded by `tick_interval_secs`.
///
/// A `scheduler_events` key is either one of the fixed names or a cron
/// expression; the expression itself becomes [`ScheduleKind::Cron`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleKind {
    /// An explicit cron expression, exactly as written.
    Cron(String),
    /// At startup and on every scheduler interval.
    All,
    /// Frappe `hourly`: every hour on the hour.
    Hourly,
    /// Frappe `daily`: midnight local time.
    Daily,
    /// Frappe `weekly`: Sunday at midnight local time.
    Weekly,
    /// Frappe `monthly`: the first day of the month at midnight.
    Monthly,
    /// Frappe `yearly`: 1 January at midnight.
    Yearly,
}

impl ScheduleKind {
    /// Parse one `scheduler_events` key.
    ///
    /// The fixed names are case-insensitive; any other key must be a valid
    /// cron expression and is kept as [`ScheduleKind::Cron`]. The bare word
    /// `cron` is rejected because it needs an expression, which the
    /// `kind -> [job]` event map has no room for.
    ///
    /// # Errors
    ///
    /// Fails when the key names neither a fixed cadence nor a valid cron
    /// expression.
    pub fn parse(key: &str) -> Result<Self> {
        let key = key.trim();
        match key.to_ascii_lowercase().as_str() {
            "all" => Ok(Self::All),
            "hourly" => Ok(Self::Hourly),
            "daily" => Ok(Self::Daily),
            "weekly" => Ok(Self::Weekly),
            "monthly" => Ok(Self::Monthly),
            "yearly" => Ok(Self::Yearly),
            "cron" => Err(Error::Validation(ValidationError::Invalid(
                "scheduler_events key `cron` needs an expression; use the expression itself as the key"
                    .to_string(),
            ))),
            _ => {
                CronExpression::validate(key)?;
                Ok(Self::Cron(key.to_string()))
            }
        }
    }

    /// The cron expression the kind expands to.
    ///
    /// `All` is an every-second expression, not an every-minute one: the tick
    /// loop is what bounds it to one run per `tick_interval_secs`, so a
    /// five-second interval really does run `All` every five seconds instead of
    /// waiting for the minute boundary. The scheduler always computes the next
    /// deadline from the current instant, so the persisted expression never
    /// replays missed ticks.
    ///
    /// # Errors
    ///
    /// Fails when an explicit [`ScheduleKind::Cron`] expression is invalid.
    pub fn to_cron(&self) -> Result<String> {
        let expression = match self {
            Self::Cron(expression) => {
                CronExpression::validate(expression)?;
                expression.clone()
            }
            Self::All => "* * * * * *".to_string(),
            Self::Hourly => "0 * * * *".to_string(),
            Self::Daily => "0 0 * * *".to_string(),
            Self::Weekly => "0 0 * * 0".to_string(),
            Self::Monthly => "0 0 1 * *".to_string(),
            Self::Yearly => "0 0 1 1 *".to_string(),
        };
        Ok(expression)
    }
}

/// One job declared by a `scheduler_events` map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledEvent {
    pub kind: ScheduleKind,
    pub job_name: String,
}

/// Parse a Frappe-style `scheduler_events` map (`kind -> [job]`).
///
/// This is the registration surface an app or a config uses: each job gets an
/// entry pointing at the cadence it asked for. Job names are trimmed and must
/// not be empty; validation of the kind is delegated to
/// [`ScheduleKind::parse`].
///
/// # Errors
///
/// Fails on an unknown/invalid kind key or an empty job name.
pub fn parse_scheduler_events(
    events: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<ScheduledEvent>> {
    let mut parsed = Vec::new();
    for (kind, jobs) in events {
        let kind = ScheduleKind::parse(kind)?;
        for job in jobs {
            let job = job.trim();
            if job.is_empty() {
                return Err(Error::Validation(ValidationError::Invalid(
                    "scheduler_events contains an empty job name".to_string(),
                )));
            }
            parsed.push(ScheduledEvent {
                kind: kind.clone(),
                job_name: job.to_string(),
            });
        }
    }
    Ok(parsed)
}

/// Parse the JSON form of a `scheduler_events` map (`{"daily": ["job"]}`).
///
/// # Errors
///
/// Fails when the JSON is not a map of string arrays, or when a kind/job name
/// is invalid.
pub fn parse_scheduler_events_json(raw: &str) -> Result<Vec<ScheduledEvent>> {
    let events: BTreeMap<String, Vec<String>> = serde_json::from_str(raw).map_err(|e| {
        Error::Validation(ValidationError::Invalid(format!(
            "scheduler_events is not a map of job-name arrays: {e}"
        )))
    })?;
    parse_scheduler_events(&events)
}

/// Normalize a cron expression for `tokio-cron-scheduler`.
///
/// That engine requires a leading **seconds** field, while Frappe uses the
/// classic 5-field (minute-first) form. A 5-field expression is prefixed with
/// `0 ` so `"0 0 * * *"` (daily at midnight) is accepted; 6- and 7-field
/// (seconds-first) expressions pass through unchanged.
pub fn normalize_cron(expr: &str) -> String {
    if expr.split_whitespace().count() == 5 {
        format!("0 {expr}")
    } else {
        expr.to_string()
    }
}

/// Cron expression validation and the `next_run` computation.
pub struct CronExpression;

impl CronExpression {
    /// Validate a cron expression with the engine's own parser.
    pub fn validate(expr: &str) -> Result<()> {
        // Use cron parser to validate (accepting both 5- and 6/7-field forms).
        let normalized = normalize_cron(expr);
        let _ = tokio_cron_scheduler::Job::new(normalized.as_str(), |_uuid, _lock| {}).map_err(
            |e| {
                Error::Validation(ValidationError::Invalid(format!(
                    "Invalid cron expression: {}",
                    e
                )))
            },
        )?;
        Ok(())
    }

    /// Get the next occurrence strictly after `from`, interpreted as UTC.
    ///
    /// Accepts the same shapes as the firing engine: 5-field (Frappe-style) or
    /// 6/7-field seconds-first, plus croner's `L` (last day of month) and `#`
    /// (nth weekday) extensions. Both sides parse with the same `croner`
    /// version, so the deadline persisted here is the moment the engine will
    /// actually fire (SCH10).
    pub fn next_occurrence(expr: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        Self::next_occurrence_in(expr, chrono_tz::UTC, from)
    }

    /// Get the next occurrence strictly after `from` in `timezone`.
    ///
    /// The expression names a wall-clock moment in `timezone` (which is what an
    /// operator configures); the result is converted back to UTC because that
    /// is what the schedules table stores. `croner` resolves DST: a fixed hour
    /// keeps its local time across a transition, and a time inside the
    /// spring-forward gap snaps to the next valid instant.
    pub fn next_occurrence_in(
        expr: &str,
        timezone: Tz,
        from: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        use std::str::FromStr;
        let cron = croner::Cron::from_str(&normalize_cron(expr)).ok()?;
        let local = from.with_timezone(&timezone);
        cron.find_next_occurrence(&local, false)
            .ok()
            .map(|next| next.with_timezone(&Utc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_creation() {
        let schedule = Schedule::new("Test Schedule".to_string(), ScheduleConfig::default())
            .with_cron("0 0 * * *".to_string())
            .with_job_name("Test Job".to_string());

        assert!(schedule.enabled);
        assert_eq!(schedule.config.cron_expression, "0 0 * * *");
    }

    #[test]
    fn test_cron_validation() {
        // 5-field Frappe-style expressions are accepted.
        assert!(CronExpression::validate("0 0 * * *").is_ok());
        assert!(CronExpression::validate("*/5 * * * *").is_ok());
        // 6-field (seconds-first) is accepted too.
        assert!(CronExpression::validate("0 0 0 * * *").is_ok());
        assert!(CronExpression::validate("invalid").is_err());
    }

    #[test]
    fn test_normalize_cron() {
        assert_eq!(normalize_cron("0 0 * * *"), "0 0 0 * * *");
        assert_eq!(normalize_cron("*/5 * * * *"), "0 */5 * * * *");
        // Already 6-field: unchanged.
        assert_eq!(normalize_cron("0 0 0 * * *"), "0 0 0 * * *");
    }

    #[test]
    fn test_next_occurrence_daily() {
        use chrono::TimeZone;
        let from = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let next = CronExpression::next_occurrence("0 0 * * *", from).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap());
    }

    /// SCHED-02: a cron expression is wall-clock time in the configured zone,
    /// while the value the scheduler stores is the UTC instant.
    #[test]
    fn next_occurrence_uses_the_configured_zone() {
        use chrono::TimeZone;
        use chrono_tz::Europe::Madrid;

        let winter_from = Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap();
        let winter = CronExpression::next_occurrence_in("0 9 * * *", Madrid, winter_from)
            .expect("winter occurrence");
        // 09:00 CET is 08:00 UTC.
        assert_eq!(winter, Utc.with_ymd_and_hms(2026, 1, 15, 8, 0, 0).unwrap());

        let summer_from = Utc.with_ymd_and_hms(2026, 7, 15, 0, 0, 0).unwrap();
        let summer = CronExpression::next_occurrence_in("0 9 * * *", Madrid, summer_from)
            .expect("summer occurrence");
        // 09:00 CEST is 07:00 UTC: the same local hour, a different instant.
        assert_eq!(summer, Utc.with_ymd_and_hms(2026, 7, 15, 7, 0, 0).unwrap());
    }

    /// SCHED-02: across both EU transitions the local hour is preserved, so
    /// consecutive daily runs are 23 h (spring forward) and 25 h (fall back)
    /// apart in UTC instead of always 24 h.
    #[test]
    fn daily_runs_keep_local_time_across_dst_transitions() {
        use chrono::TimeZone;
        use chrono_tz::Europe::Madrid;

        // 2026-03-29 is the spring-forward Sunday (CET -> CEST).
        let before = Utc.with_ymd_and_hms(2026, 3, 28, 0, 0, 0).unwrap();
        let first = CronExpression::next_occurrence_in("0 9 * * *", Madrid, before).unwrap();
        let second = CronExpression::next_occurrence_in("0 9 * * *", Madrid, first).unwrap();
        assert_eq!(
            (second - first).num_hours(),
            23,
            "the 09:00 wall-clock run must shift by one UTC hour on spring forward"
        );

        // 2026-10-25 is the fall-back Sunday (CEST -> CET).
        let before = Utc.with_ymd_and_hms(2026, 10, 24, 0, 0, 0).unwrap();
        let first = CronExpression::next_occurrence_in("0 9 * * *", Madrid, before).unwrap();
        let second = CronExpression::next_occurrence_in("0 9 * * *", Madrid, first).unwrap();
        assert_eq!(
            (second - first).num_hours(),
            25,
            "the 09:00 wall-clock run must repeat an UTC hour on fall back"
        );
    }

    /// A time inside the spring-forward gap has no local instant; croner snaps
    /// it to the first valid moment after the gap instead of returning `None`.
    #[test]
    fn a_gap_time_resolves_after_the_spring_forward_gap() {
        use chrono::TimeZone;
        use chrono_tz::Europe::Madrid;

        let before = Utc.with_ymd_and_hms(2026, 3, 28, 23, 0, 0).unwrap();
        let next = CronExpression::next_occurrence_in("30 2 * * *", Madrid, before)
            .expect("a gapped local time still yields an instant");
        // 02:30 on 2026-03-29 does not exist; the resolved instant must be
        // valid local time on that date and not before the gap.
        let local = next.with_timezone(&Madrid);
        assert_eq!(local.date_naive().to_string(), "2026-03-29");
        assert!(
            next >= Utc.with_ymd_and_hms(2026, 3, 29, 1, 0, 0).unwrap(),
            "resolved {next} is before the 01:00 UTC gap boundary"
        );
    }

    #[test]
    fn kind_parses_the_frappe_names_and_cron_expressions() {
        assert_eq!(ScheduleKind::parse("all").unwrap(), ScheduleKind::All);
        assert_eq!(ScheduleKind::parse("Hourly").unwrap(), ScheduleKind::Hourly);
        assert_eq!(ScheduleKind::parse("daily").unwrap(), ScheduleKind::Daily);
        assert_eq!(ScheduleKind::parse("weekly").unwrap(), ScheduleKind::Weekly);
        assert_eq!(
            ScheduleKind::parse("monthly").unwrap(),
            ScheduleKind::Monthly
        );
        assert_eq!(ScheduleKind::parse("yearly").unwrap(), ScheduleKind::Yearly);
        assert_eq!(
            ScheduleKind::parse("*/5 * * * *").unwrap(),
            ScheduleKind::Cron("*/5 * * * *".to_string())
        );
        assert!(ScheduleKind::parse("not-a-cadence").is_err());
        assert!(ScheduleKind::parse("cron").is_err());
    }

    #[test]
    fn kind_converts_to_the_frappe_cron_equivalents() {
        assert_eq!(ScheduleKind::All.to_cron().unwrap(), "* * * * * *");
        assert_eq!(ScheduleKind::Hourly.to_cron().unwrap(), "0 * * * *");
        assert_eq!(ScheduleKind::Daily.to_cron().unwrap(), "0 0 * * *");
        assert_eq!(ScheduleKind::Weekly.to_cron().unwrap(), "0 0 * * 0");
        assert_eq!(ScheduleKind::Monthly.to_cron().unwrap(), "0 0 1 * *");
        assert_eq!(ScheduleKind::Yearly.to_cron().unwrap(), "0 0 1 1 *");
        assert_eq!(
            ScheduleKind::Cron("*/5 * * * *".to_string())
                .to_cron()
                .unwrap(),
            "*/5 * * * *"
        );
    }

    #[test]
    fn scheduler_events_map_expands_every_job() {
        let events: BTreeMap<String, Vec<String>> = BTreeMap::from([
            (
                "daily".to_string(),
                vec![" nightly".to_string(), "digest".to_string()],
            ),
            ("*/5 * * * *".to_string(), vec!["poll".to_string()]),
        ]);
        let parsed = parse_scheduler_events(&events).expect("valid events");
        assert_eq!(parsed.len(), 3);
        let nightly = parsed
            .iter()
            .find(|event| event.job_name == "nightly")
            .expect("the trimmed daily job");
        assert_eq!(nightly.kind, ScheduleKind::Daily);
        assert!(
            parsed
                .iter()
                .any(|event| event.kind == ScheduleKind::Cron("*/5 * * * *".to_string()))
        );
    }

    #[test]
    fn scheduler_events_json_is_the_same_map() {
        let parsed = parse_scheduler_events_json(r#"{"hourly": ["sweep"], "daily": ["report"]}"#)
            .expect("valid JSON");
        assert_eq!(parsed.len(), 2);
        assert!(parse_scheduler_events_json(r#"{"bogus": ["x"]}"#).is_err());
        assert!(parse_scheduler_events_json(r#"{"daily": [""]}"#).is_err());
    }

    /// A12: a persisted `next_run` that is far enough in the past marks an
    /// execution missed while the process was down — the scheduler must
    /// recover it on boot instead of dropping it silently.
    #[test]
    fn test_missed_while_down_detects_an_overdue_deadline() {
        use chrono::TimeZone;
        let now = Utc.with_ymd_and_hms(2026, 1, 3, 12, 0, 0).unwrap();
        let schedule = Schedule::new("x".to_string(), ScheduleConfig::default());

        // Deadline an hour ago, enabled → missed.
        let mut missed = schedule.clone();
        missed.next_run = Some(now - chrono::Duration::hours(1));
        assert!(missed.missed_while_down(now, 60));

        // Disabled schedules never "miss" — they are not supposed to run.
        let mut disabled = schedule.clone();
        disabled.next_run = Some(now - chrono::Duration::hours(1));
        disabled.enabled = false;
        assert!(!disabled.missed_while_down(now, 60));

        // Inside the misfire grace: too recent, likely just consumed by the
        // engine in another process — don't run it twice (A12 coalescing).
        let mut fresh = schedule.clone();
        fresh.next_run = Some(now - chrono::Duration::seconds(10));
        assert!(!fresh.missed_while_down(now, 60));

        // No deadline computed yet: not missed, just unknown.
        assert!(!schedule.missed_while_down(now, 60));
    }

    #[test]
    fn test_monthly_last_is_last_day() {
        use chrono::TimeZone;
        let from = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
        let next = CronExpression::next_occurrence("0 0 L * *", from).unwrap();
        // 2026-02 has 28 days; `28-31` would have matched the 28th too, but so
        // does `L`. The important part is it never fires in March for February.
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 2, 28, 0, 0, 0).unwrap());
    }
}
