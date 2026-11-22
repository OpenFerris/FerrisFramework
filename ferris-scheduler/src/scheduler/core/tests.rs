//! `Scheduler` behaviour: ids, the in-memory mirror, the persist-then-register
//! ordering and multi-instance leadership.

use super::*;
use ferris_jobs::QueueConfig;

async fn scheduler() -> Scheduler {
    let queue = Arc::new(JobQueue::new_db(
        QueueConfig::default(),
        ferris_db::memory_db().await.unwrap(),
    ));
    queue.init().await.unwrap();
    Scheduler::new(queue, ferris_db::memory_db().await.unwrap())
        .await
        .unwrap()
}

fn schedule(name: &str) -> Schedule {
    Schedule::new(
        name.to_string(),
        ScheduleConfig {
            cron_expression: "0 * * * *".to_string(),
            job_name: "send_digest".to_string(),
            ..Default::default()
        },
    )
}

/// SCH1: the returned id must be the id of the schedule that was added, not
/// whichever key the `HashMap` iterator happens to end on.
#[tokio::test]
async fn adding_two_schedules_returns_each_new_id() {
    let scheduler = scheduler().await;
    let first = schedule("first");
    let second = schedule("second");
    let first_id = first.id.clone();
    let second_id = second.id.clone();

    assert_eq!(scheduler.add_schedule(first).await.unwrap(), first_id);
    assert_eq!(scheduler.add_schedule(second).await.unwrap(), second_id);
}

/// SCH3: a run's persisted counters and error must reach the in-memory read
/// model, otherwise `list_schedules` reports zeros until the next restart.
#[tokio::test]
async fn a_run_outcome_is_mirrored_into_the_in_memory_schedule() {
    let scheduler = scheduler().await;
    let nightly = schedule("nightly");
    let id = nightly.id.clone();
    scheduler.add_schedule(nightly).await.unwrap();

    scheduler
        .store
        .update_run_info(
            &id,
            Utc::now(),
            Some(Utc::now() + chrono::Duration::hours(1)),
            false,
            Some("boom".to_string()),
        )
        .await
        .unwrap();
    refresh_schedule(&scheduler.store, &scheduler.schedules, &id).await;

    let mirrored = scheduler.get_schedule(&id).await.unwrap().unwrap();
    assert_eq!(mirrored.run_count, 1);
    assert_eq!(mirrored.failure_count, 1);
    assert_eq!(mirrored.last_error.as_deref(), Some("boom"));
    assert!(mirrored.last_run.is_some());
    assert!(mirrored.next_run.is_some());
}

/// SCH11: when the row cannot be written, the schedule must not be left
/// registered with the engine or visible in memory.
#[tokio::test]
async fn a_schedule_that_cannot_be_persisted_is_not_registered() {
    let scheduler = scheduler().await;
    scheduler
        .store
        .drop_table_for_test()
        .await
        .expect("drop schedules table");

    let error = scheduler
        .add_schedule(schedule("nightly"))
        .await
        .expect_err("a failed persist must fail the add");
    assert!(matches!(error, Error::Database(_)));
    assert!(scheduler.schedules.read().await.is_empty());
    assert!(scheduler.registered.read().await.is_empty());
}

/// SCH11: a disabled schedule is persisted but never registered, and a
/// re-enable registers it exactly once.
#[tokio::test]
async fn a_disabled_schedule_is_persisted_without_a_cron_job() {
    let scheduler = scheduler().await;
    let disabled = schedule("paused").disabled();
    let id = disabled.id.clone();
    scheduler.add_schedule(disabled).await.unwrap();

    assert!(scheduler.registered.read().await.is_empty());
    assert!(!scheduler.get_schedule(&id).await.unwrap().unwrap().enabled);

    scheduler.enable_schedule(&id).await.unwrap();
    assert_eq!(scheduler.registered.read().await.len(), 1);
    scheduler.enable_schedule(&id).await.unwrap();
    assert_eq!(
        scheduler.registered.read().await.len(),
        1,
        "enabling twice must not register a second cron job"
    );

    scheduler.disable_schedule(&id).await.unwrap();
    assert!(scheduler.registered.read().await.is_empty());
    assert!(!scheduler.get_schedule(&id).await.unwrap().unwrap().enabled);
}

/// Build a queue and the shared database the scheduler tests run on.
async fn shared_db() -> (Arc<JobQueue>, ferris_db::Db) {
    let db = ferris_db::memory_db().await.expect("in-memory db");
    let queue = Arc::new(JobQueue::new_db(QueueConfig::default(), db.clone()));
    queue.init().await.expect("queue init");
    (queue, db)
}

/// An overdue schedule persisted straight into the store, i.e. one whose run
/// was missed while no scheduler was leading.
async fn seed_overdue(db: &ferris_db::Db) -> String {
    let store = ScheduleStore::new(db.clone());
    store.init().await.expect("store init");
    let mut overdue = schedule("overdue");
    overdue.next_run = Some(Utc::now() - chrono::Duration::hours(1));
    let id = overdue.id.clone();
    store.save(&overdue).await.expect("save overdue schedule");
    id
}

/// The multi-instance guarantee: two schedulers over one database, one lease,
/// one enqueued run. Before the lease every process loaded every schedule and
/// enqueued its own copy per tick.
#[tokio::test]
async fn only_one_of_two_schedulers_enqueues_a_missed_run() {
    let (queue, db) = shared_db().await;
    seed_overdue(&db).await;

    let first = Scheduler::with_config(queue.clone(), db.clone(), &SchedulerConfig::default())
        .await
        .expect("first scheduler");
    let second = Scheduler::with_config(queue.clone(), db.clone(), &SchedulerConfig::default())
        .await
        .expect("second scheduler");

    first.start().await.expect("first start");
    second.start().await.expect("second start");

    assert!(
        first.is_leader() != second.is_leader(),
        "exactly one instance must hold the lease"
    );

    // Long enough for a spurious second catch-up from the initial acquisition,
    // far shorter than the lease TTL (so no takeover can happen here).
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let jobs = queue
        .list_jobs(None, Some("default".to_string()), 10)
        .await
        .expect("list jobs");
    assert_eq!(
        jobs.len(),
        1,
        "the missed run must be enqueued exactly once, got {jobs:?}"
    );

    first.shutdown().await.expect("first shutdown");
    second.shutdown().await.expect("second shutdown");
}

/// A follower takes over once the holder's lease expires, and performs the
/// catch-up the dead leader never did.
#[tokio::test]
async fn a_follower_takes_over_an_expired_lease_and_catches_up() {
    let (queue, db) = shared_db().await;
    let schedule_id = seed_overdue(&db).await;

    // A one-second tick gives a short acquisition retry, so the takeover is
    // observed in a test rather than after half a minute.
    let config = SchedulerConfig {
        tick_interval_secs: 1,
        ..Default::default()
    };
    let leader = Scheduler::with_config(queue.clone(), db.clone(), &config)
        .await
        .expect("leader scheduler");
    let follower = Scheduler::with_config(queue.clone(), db.clone(), &config)
        .await
        .expect("follower scheduler");

    leader.start().await.expect("leader start");
    assert!(leader.is_leader(), "the first instance is the leader");
    follower.start().await.expect("follower start");
    assert!(!follower.is_leader(), "the second instance waits");

    // The leader's catch-up rolled the deadline forward. Put it back into the
    // past: the leader is about to die with a run still pending.
    let mut stored = leader
        .store
        .get(&schedule_id)
        .await
        .expect("read schedule")
        .expect("schedule exists");
    stored.next_run = Some(Utc::now() - chrono::Duration::hours(1));
    leader.store.save(&stored).await.expect("rewrite deadline");

    // Simulate a crashed holder instead of waiting out the TTL: stop its
    // loops so it neither renews the lease nor fires its tick, then make the
    // persisted deadline stale. (A real crash kills the process, which stops
    // both.)
    let lease_task = leader.lease_task.lock().await.take();
    if let Some(task) = lease_task {
        task.abort();
    }
    let tick_task = leader.tick_task.lock().await.take();
    if let Some(task) = tick_task {
        task.abort();
    }
    leader.is_leader.store(false, Ordering::SeqCst);
    leader.lease.expire_for_test().await.expect("expire lease");

    let takeover_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !follower.is_leader() {
        assert!(
            std::time::Instant::now() < takeover_deadline,
            "the follower never took over the expired lease"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // The takeover triggers the catch-up of the run the old leader missed.
    let catch_up_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let jobs = queue
            .list_jobs(None, Some("default".to_string()), 10)
            .await
            .expect("list jobs");
        if jobs.len() == 2 {
            break;
        }
        assert!(
            std::time::Instant::now() < catch_up_deadline,
            "the new leader never caught up the missed run (jobs: {})",
            jobs.len()
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    leader.shutdown().await.expect("leader shutdown");
    follower.shutdown().await.expect("follower shutdown");
}

/// `shutdown` releases the lease, so a surviving instance does not have to
/// wait for the TTL before it can lead.
#[tokio::test]
async fn shutting_down_hands_the_lease_to_a_follower() {
    let (queue, db) = shared_db().await;
    let config = SchedulerConfig {
        tick_interval_secs: 1,
        ..Default::default()
    };
    let first = Scheduler::with_config(queue.clone(), db.clone(), &config)
        .await
        .expect("first scheduler");
    let second = Scheduler::with_config(queue.clone(), db.clone(), &config)
        .await
        .expect("second scheduler");

    first.start().await.expect("first start");
    second.start().await.expect("second start");
    assert!(first.is_leader());
    assert!(!second.is_leader());

    first.shutdown().await.expect("first shutdown");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !second.is_leader() {
        assert!(
            std::time::Instant::now() < deadline,
            "the follower did not pick up the released lease"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    second.shutdown().await.expect("second shutdown");
}

/// A corrupt row must not stop the scheduler from loading every healthy one.
#[tokio::test]
async fn a_corrupt_schedule_row_does_not_prevent_booting() {
    let (queue, db) = shared_db().await;
    let good = schedule("healthy");
    let good_id = good.id.clone();
    let broken = schedule("corrupt");
    let broken_id = broken.id.clone();

    let store = ScheduleStore::new(db.clone());
    store.init().await.expect("store init");
    store.save(&good).await.expect("save good");
    store.save(&broken).await.expect("save broken");
    // A legacy/garbage deadline that no parser accepts.
    db.execute(&ferris_db::Stmt::new(
        "UPDATE schedules SET next_run = ? WHERE id = ?",
        vec![
            ferris_core::Value::from("not-a-timestamp"),
            ferris_core::Value::from(broken_id.as_str()),
        ],
    ))
    .await
    .expect("corrupt the row");

    let scheduler = Scheduler::new(queue, db).await.expect("scheduler boots");
    let loaded = scheduler.list_schedules().await.expect("list");
    assert_eq!(loaded.len(), 1, "the healthy schedule still loads");
    assert_eq!(loaded[0].id, good_id);
}

/// SCHED-01: production passes `settings.config.scheduler`, and every knob must
/// be the effective one inside the instance — not just a logged string.
#[tokio::test]
async fn with_config_applies_tick_concurrency_and_retention() {
    let (queue, db) = shared_db().await;
    let config = SchedulerConfig {
        tick_interval_secs: 7,
        max_concurrent_jobs: 3,
        history_retention_days: 11,
        timezone: "Europe/Madrid".to_string(),
        ..SchedulerConfig::default()
    };

    let scheduler = Scheduler::with_config(queue, db, &config)
        .await
        .expect("scheduler with config");

    assert_eq!(scheduler.config().tick_interval_secs, 7);
    assert_eq!(scheduler.config().max_concurrent_jobs, 3);
    assert_eq!(scheduler.config().history_retention_days, 11);
    assert_eq!(scheduler.config().timezone, "Europe/Madrid");
    assert_eq!(scheduler.timezone(), chrono_tz::Europe::Madrid);
    assert_eq!(
        scheduler.enqueue_slots.available_permits(),
        3,
        "the semaphore must be sized from max_concurrent_jobs"
    );
    assert_eq!(
        scheduler.settings.tick,
        std::time::Duration::from_secs(7),
        "the tick loop must run at tick_interval_secs"
    );
}

/// A zone the operator typed wrong must fail the boot instead of silently
/// scheduling in UTC (no silent success).
#[tokio::test]
async fn with_config_rejects_an_unknown_timezone() {
    let (queue, db) = shared_db().await;
    let config = SchedulerConfig {
        timezone: "Mars/Olympus".to_string(),
        ..SchedulerConfig::default()
    };

    let error = Scheduler::with_config(queue, db, &config)
        .await
        .err()
        .expect("an unknown zone must fail");
    assert!(matches!(error, Error::Validation(_)));
}

/// SCHED-02: a schedule added to a zone-configured scheduler stores a UTC
/// instant whose local projection is the wall-clock hour the cron names.
#[tokio::test]
async fn next_run_is_computed_in_the_configured_zone() {
    use chrono::Timelike;
    let (queue, db) = shared_db().await;
    let config = SchedulerConfig {
        timezone: "Europe/Madrid".to_string(),
        ..SchedulerConfig::default()
    };
    let scheduler = Scheduler::with_config(queue, db, &config)
        .await
        .expect("scheduler");

    let nightly = schedule("nightly").with_cron("0 9 * * *".to_string());
    let id = nightly.id.clone();
    scheduler.add_schedule(nightly).await.expect("add");

    let stored = scheduler
        .get_schedule(&id)
        .await
        .expect("read")
        .expect("exists");
    let local = stored
        .next_run
        .expect("a persisted deadline")
        .with_timezone(&chrono_tz::Europe::Madrid);
    assert_eq!((local.hour(), local.minute()), (9, 0));
}

/// SCHED-04: the catch-up renews the lease before every run it recovers. Once
/// another instance owns the lease, the old holder must enqueue nothing.
#[tokio::test]
async fn a_leader_that_lost_the_lease_does_not_enqueue_the_catch_up() {
    let (queue, db) = shared_db().await;
    seed_overdue(&db).await;

    let scheduler = Scheduler::with_config(
        queue.clone(),
        db.clone(),
        &SchedulerConfig {
            tick_interval_secs: 1,
            ..SchedulerConfig::default()
        },
    )
    .await
    .expect("scheduler");

    // A successor takes over after the deadline lapses; renewing then fails
    // because the row's owner changed.
    let successor = crate::leader::LeaderLease::new(db.clone(), LEASE_NAME, "successor", 30);
    scheduler.lease.expire_for_test().await.expect("expire");
    assert!(
        successor.try_acquire().await.expect("successor acquires"),
        "the successor must win the expired lease"
    );

    let still_leading = scheduler.duties.on_became_leader().await;
    assert!(!still_leading, "a lost lease cannot be renewed");

    let jobs = queue
        .list_jobs(None, Some("default".to_string()), 10)
        .await
        .expect("list jobs");
    assert!(
        jobs.is_empty(),
        "the deposed leader must not enqueue the backlog, got {jobs:?}"
    );
}

/// SCHED-04: takeover with a backlog. The new leader recovers the missed run
/// once; the deposed leader's own catch-up attempt is fenced out.
#[tokio::test]
async fn a_takeover_with_a_backlog_enqueues_once_and_fences_the_old_leader() {
    let (queue, db) = shared_db().await;
    let schedule_id = seed_overdue(&db).await;
    let config = SchedulerConfig {
        tick_interval_secs: 1,
        ..SchedulerConfig::default()
    };

    let leader = Scheduler::with_config(queue.clone(), db.clone(), &config)
        .await
        .expect("leader");
    let follower = Scheduler::with_config(queue.clone(), db.clone(), &config)
        .await
        .expect("follower");
    leader.start().await.expect("leader start");
    follower.start().await.expect("follower start");
    assert!(leader.is_leader());
    assert!(!follower.is_leader());

    // The leader's boot catch-up consumed the overdue run. Put the deadline
    // back: the leader is about to die with a run still pending.
    let mut stored = leader
        .store
        .get(&schedule_id)
        .await
        .expect("read schedule")
        .expect("schedule exists");
    stored.next_run = Some(Utc::now() - chrono::Duration::hours(1));
    leader.store.save(&stored).await.expect("rewrite deadline");

    // Simulate a crash: stop both tasks (no renewal, no tick) and let the
    // persisted deadline lapse. A dead process has neither loop running.
    let lease_task = leader.lease_task.lock().await.take();
    if let Some(task) = lease_task {
        task.abort();
    }
    let tick_task = leader.tick_task.lock().await.take();
    if let Some(task) = tick_task {
        task.abort();
    }
    leader.is_leader.store(false, Ordering::SeqCst);
    leader.lease.expire_for_test().await.expect("expire lease");

    let takeover_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !follower.is_leader() {
        assert!(
            std::time::Instant::now() < takeover_deadline,
            "the follower never took over the expired lease"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let catch_up_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let jobs = queue
            .list_jobs(None, Some("default".to_string()), 10)
            .await
            .expect("list jobs");
        if jobs.len() == 2 {
            break;
        }
        assert!(
            std::time::Instant::now() < catch_up_deadline,
            "the new leader never caught up the missed run (jobs: {})",
            jobs.len()
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // The deposed leader keeps trying: its renewal fails, so it must not add a
    // third copy of the same recovered run.
    let still_leading = leader.duties.on_became_leader().await;
    assert!(!still_leading);
    let jobs = queue
        .list_jobs(None, Some("default".to_string()), 10)
        .await
        .expect("list jobs");
    assert_eq!(jobs.len(), 2, "the fenced leader enqueued a duplicate");

    leader.shutdown().await.expect("leader shutdown");
    follower.shutdown().await.expect("follower shutdown");
}

/// SCHED-07: a `scheduler_events` map becomes schedules, `all` is due
/// immediately, and registering twice does not duplicate rows.
#[tokio::test]
async fn scheduler_events_register_kinds_and_are_idempotent() {
    let scheduler = scheduler().await;
    let events = std::collections::BTreeMap::from([
        ("daily".to_string(), vec!["nightly".to_string()]),
        ("all".to_string(), vec!["boot".to_string()]),
        ("*/5 * * * *".to_string(), vec!["poll".to_string()]),
    ]);

    let ids = scheduler
        .register_scheduler_events(&events, "default")
        .await
        .expect("register events");
    assert_eq!(ids.len(), 3);

    let schedules = scheduler.list_schedules().await.expect("list");
    assert_eq!(schedules.len(), 3);

    let all = schedules
        .iter()
        .find(|schedule| schedule.config.job_name == "boot")
        .expect("the all event");
    assert_eq!(all.config.cron_expression, "* * * * * *");
    assert!(
        all.next_run.is_some_and(|next| next <= Utc::now()),
        "`all` must be due at startup"
    );

    let again = scheduler
        .register_scheduler_events(&events, "default")
        .await
        .expect("re-register");
    assert!(again.is_empty(), "existing job names are left untouched");
    assert_eq!(scheduler.list_schedules().await.expect("list").len(), 3);
}
