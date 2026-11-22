//! `ScheduleStore` round trip, run accounting and corruption reporting.

use super::store::TAB_SCHEDULES;
use super::*;
use chrono::Utc;
use ferris_core::{Error, ValidationError, Value};
use ferris_db::Stmt;

async fn store() -> ScheduleStore {
    let store = ScheduleStore::new(ferris_db::memory_db().await.unwrap());
    store.init().await.unwrap();
    store
}

fn schedule(name: &str) -> crate::schedule::Schedule {
    let mut schedule = crate::schedule::Schedule::new(
        name.to_string(),
        crate::schedule::ScheduleConfig {
            cron_expression: "0 * * * *".to_string(),
            job_name: "send_digest".to_string(),
            payload: Value::from("body"),
            tags: vec!["nightly".to_string()],
            ..Default::default()
        },
    );
    schedule.next_run = Some(Utc::now() - chrono::Duration::minutes(1));
    schedule
}

#[test]
fn test_schedule_record_conversion() {
    let schedule = crate::schedule::Schedule::new(
        "Test".to_string(),
        crate::schedule::ScheduleConfig::default(),
    );

    let record = ScheduleRecord::from_schedule(&schedule).unwrap();
    let converted = record.into_schedule().unwrap();

    assert_eq!(converted.id, schedule.id);
    assert_eq!(converted.name, schedule.name);
}

#[tokio::test]
async fn a_schedule_survives_the_round_trip() {
    let store = store().await;
    let original = schedule("nightly digest");
    store.save(&original).await.unwrap();

    let loaded = store.get(&original.id).await.unwrap().unwrap();
    assert_eq!(loaded.name, "nightly digest");
    assert_eq!(loaded.config.cron_expression, "0 * * * *");
    assert_eq!(loaded.config.payload.as_str(), Some("body"));
    assert_eq!(loaded.config.tags, vec!["nightly".to_string()]);
    assert!(loaded.enabled);
    assert!(loaded.next_run.is_some());
}

#[tokio::test]
async fn saving_twice_updates_rather_than_duplicates() {
    let store = store().await;
    let mut original = schedule("nightly digest");
    store.save(&original).await.unwrap();

    original.enabled = false;
    original.config.job_name = "send_report".to_string();
    store.save(&original).await.unwrap();

    let all = store.list().await.unwrap();
    assert_eq!(all.len(), 1);
    assert!(!all[0].enabled);
    let loaded = store.get(&original.id).await.unwrap().unwrap();
    assert!(!loaded.enabled);
    assert_eq!(loaded.config.job_name, "send_report");
}

#[tokio::test]
async fn a_failed_run_counts_and_records_the_error() {
    let store = store().await;
    let original = schedule("nightly digest");
    store.save(&original).await.unwrap();

    store
        .update_run_info(
            &original.id,
            Utc::now(),
            None,
            false,
            Some("boom".to_string()),
        )
        .await
        .unwrap();

    let loaded = store.get(&original.id).await.unwrap().unwrap();
    assert_eq!(loaded.run_count, 1);
    assert_eq!(loaded.failure_count, 1);
    assert_eq!(loaded.last_error.as_deref(), Some("boom"));

    // A run with no error message still has to bind the `last_error`
    // placeholder the failure branch declares.
    store
        .update_run_info(&original.id, Utc::now(), None, false, None)
        .await
        .unwrap();
    let loaded = store.get(&original.id).await.unwrap().unwrap();
    assert_eq!(loaded.failure_count, 2);
    assert!(loaded.last_error.is_none());

    // A success clears both.
    store
        .update_run_info(&original.id, Utc::now(), None, true, None)
        .await
        .unwrap();
    let loaded = store.get(&original.id).await.unwrap().unwrap();
    assert_eq!(loaded.run_count, 3);
    assert_eq!(loaded.failure_count, 0);
    assert!(loaded.last_error.is_none());
}

#[tokio::test]
async fn deleting_removes_the_row() {
    let store = store().await;
    let original = schedule("nightly digest");
    store.save(&original).await.unwrap();

    store.delete(&original.id).await.unwrap();
    assert!(store.get(&original.id).await.unwrap().is_none());
    assert!(store.list().await.unwrap().is_empty());
}

/// SCH13: recording a run of a schedule that no longer exists must fail.
/// The old update matched zero rows and reported success, so the caller
/// believed the counters had moved.
#[tokio::test]
async fn recording_a_run_of_an_unknown_schedule_reports_not_found() {
    let store = store().await;
    let error = store
        .update_run_info("missing", Utc::now(), None, true, None)
        .await
        .expect_err("an unknown schedule must not report success");
    assert!(matches!(
        error,
        Error::Job(ferris_core::JobError::NotFound(id)) if id == "missing"
    ));
}

/// SCH7: a corrupt timestamp must be reported, not replaced with "now".
/// Replacing `next_run` with the current instant made the deadline look
/// freshly scheduled, so the A12 missed-run recovery never fired.
#[tokio::test]
async fn a_corrupt_timestamp_is_reported_instead_of_becoming_now() {
    let store = store().await;
    let original = schedule("nightly digest");
    store.save(&original).await.unwrap();

    store
        .db
        .execute(&Stmt::new(
            format!("UPDATE {TAB_SCHEDULES} SET next_run = ? WHERE id = ?"),
            vec![
                Value::from("not-a-timestamp"),
                Value::from(original.id.as_str()),
            ],
        ))
        .await
        .unwrap();

    let error = store
        .get(&original.id)
        .await
        .expect_err("a corrupt deadline must not be silently rewritten");
    assert!(matches!(
        error,
        Error::Validation(ValidationError::Invalid(message))
            if message.contains("next_run")
    ));
}

/// A single corrupt row must not hide every healthy schedule: `list` skips it
/// (with a warning) instead of failing the whole read, which used to prevent
/// the scheduler from booting.
#[tokio::test]
async fn a_corrupt_row_is_skipped_and_the_healthy_ones_are_listed() {
    let store = store().await;
    let good = schedule("healthy");
    let broken = schedule("corrupt");
    store.save(&good).await.unwrap();
    store.save(&broken).await.unwrap();

    store
        .db
        .execute(&Stmt::new(
            format!("UPDATE {TAB_SCHEDULES} SET last_run = ? WHERE id = ?"),
            vec![
                Value::from("not-a-timestamp"),
                Value::from(broken.id.as_str()),
            ],
        ))
        .await
        .unwrap();

    let listed = store
        .list()
        .await
        .expect("one corrupt row must not fail the listing");
    assert_eq!(listed.len(), 1, "only the healthy schedule is listed");
    assert_eq!(listed[0].id, good.id);

    // The corrupt row is still reported when asked for directly, so it does
    // not silently disappear from the operator's view.
    assert!(store.get(&broken.id).await.is_err());
}
