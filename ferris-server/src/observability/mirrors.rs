//! State-driven observability: periodic snapshots and store mirrors.
//!
//! Each mirror is gated by the feature of the store it reads: `jobs` for the
//! queue/worker rows, `scheduler` for the cron rows and `report` for the
//! prepared-report rows. The snapshots that need no optional store (version,
//! health, storage usage) always run, so even an `api`-only process writes the
//! rows its own health page needs.

use ferris_core::Value;
use ferris_db::{Db, Dialect, Stmt};
#[cfg(feature = "jobs")]
use ferris_document::chrono::DateTime;
use ferris_document::chrono::Utc;
use ferris_document::engine::{DocumentEngine, ListQuery};
#[cfg(feature = "jobs")]
use ferris_jobs::{JobQueue, JobStatus};
#[cfg(feature = "report")]
#[cfg(feature = "scheduler")]
use ferris_scheduler::Scheduler;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use super::{Mirrors, SNAPSHOT_INTERVAL, Sources, system_ctx, upsert, write_doc, write_row};

/// Spawn the state-driven snapshot task.
///
/// The first tick fires immediately, which is what writes the `Version` row and
/// the boot health/storage snapshot. The task owns its mirror bookkeeping: a
/// logical key (job id, worker, schedule id…) maps to the document name the
/// engine minted, so later ticks update instead of inserting duplicates.
#[must_use]
pub fn spawn(pool: Db, sources: Sources) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut mirrors = Mirrors::default();
        let mut ticker = tokio::time::interval(SNAPSHOT_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(error) = snapshot_once(&pool, &sources, &mut mirrors).await {
                warn!(%error, "system observability snapshot failed");
            }
        }
    })
}

async fn snapshot_once(
    pool: &Db,
    sources: &Sources,
    mirrors: &mut Mirrors,
) -> ferris_core::Result<()> {
    write_version_once(pool, mirrors).await?;

    #[cfg(feature = "jobs")]
    if let Some(queue) = sources.queue.as_deref() {
        write_rq_jobs(queue, pool, mirrors).await;
        write_rq_workers(queue, pool, mirrors).await;
    }
    #[cfg(feature = "scheduler")]
    if let Some(scheduler) = sources.scheduler.as_deref() {
        write_schedules(scheduler, pool, mirrors).await;
    }
    Ok(())
}

/// Insert the running version once per version string.
async fn write_version_once(pool: &Db, mirrors: &mut Mirrors) -> ferris_core::Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    if mirrors.names.contains_key(&format!("Version:{version}")) {
        return Ok(());
    }
    let mut query = ListQuery {
        limit: Some(1),
        ..Default::default()
    };
    query
        .filters
        .insert("version".to_string(), Value::from(version));
    let ctx = system_ctx("Version");
    let engine = DocumentEngine::new(pool.clone());
    let (rows, _) = engine.list(&ctx, &query).await?;
    if !rows.is_empty() {
        mirrors
            .names
            .insert(format!("Version:{version}"), String::new());
        return Ok(());
    }
    let commit = option_env!("FERRIS_BUILD_COMMIT").unwrap_or("unknown");
    let name = write_doc(
        pool,
        "Version",
        serde_json::json!({
            "version": version,
            "released_on": Utc::now().date_naive().to_string(),
            "commit": commit,
            "notes": "row written at boot by the System observability producer",
        }),
    )
    .await?;
    mirrors.names.insert(format!("Version:{version}"), name);
    Ok(())
}

/// The live health probe `GET /api/system/health` serves.
///
/// The probe is intentionally cheap: if the pool cannot answer `SELECT 1`, the
/// process is Down for every practical purpose. It is computed on demand
/// instead of persisted every five minutes: the old `System Health Report`
/// row-per-tick was write-only and grew without retention.
pub(crate) async fn health_snapshot(pool: &Db) -> serde_json::Value {
    let database = pool
        .fetch_optional(&Stmt::new("SELECT 1", vec![]))
        .await
        .is_ok();
    serde_json::json!({
        "status": if database { "Healthy" } else { "Down" },
        "database": if database { "ok" } else { "unreachable" },
        "version": env!("CARGO_PKG_VERSION"),
        "checked_at": Utc::now().to_rfc3339(),
    })
}

/// The live per-table rows and sizes `GET /api/system/storage` serves.
pub(crate) async fn storage_snapshot(pool: &Db) -> ferris_core::Result<Vec<TableUsage>> {
    table_usage(pool).await
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct TableUsage {
    pub name: String,
    pub rows: i64,
    pub size_mb: f64,
}

async fn table_usage(pool: &Db) -> ferris_core::Result<Vec<TableUsage>> {
    match pool.dialect() {
        Dialect::Postgres => {
            let rows = pool
                .fetch_all(&Stmt::new(
                    "SELECT relname AS table_name, n_live_tup AS rows, \
                     pg_total_relation_size(relid)::double precision / 1048576.0 AS size_mb \
                     FROM pg_stat_user_tables ORDER BY relname",
                    vec![],
                ))
                .await?;
            Ok(rows
                .iter()
                .map(|row| TableUsage {
                    name: row.str("table_name").unwrap_or_default(),
                    rows: row.i64("rows").unwrap_or(0),
                    size_mb: row.opt_f64("size_mb").unwrap_or(0.0),
                })
                .collect())
        }
        Dialect::MySQL => {
            let rows = pool
                .fetch_all(&Stmt::new(
                    "SELECT table_name AS table_name, table_rows AS rows, \
                     (data_length + index_length) / 1048576.0 AS size_mb \
                     FROM information_schema.tables \
                     WHERE table_schema = DATABASE() ORDER BY table_name",
                    vec![],
                ))
                .await?;
            Ok(rows
                .iter()
                .map(|row| TableUsage {
                    name: row.str("table_name").unwrap_or_default(),
                    rows: row.i64("rows").unwrap_or(0),
                    size_mb: row.opt_f64("size_mb").unwrap_or(0.0),
                })
                .collect())
        }
        Dialect::SQLite => {
            let tables = pool
                .fetch_all(&Stmt::new(
                    "SELECT name FROM sqlite_master \
                     WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
                    vec![],
                ))
                .await?;
            let mut usage = Vec::with_capacity(tables.len());
            for row in &tables {
                let Ok(name) = row.str("name") else {
                    continue;
                };
                // Table names come from the database catalog, not from a
                // request; they still cannot be bound as identifiers, so they
                // go through ferris-db's quoting helper.
                let quoted = ferris_db::sql::Ident::new(&name)
                    .map_or_else(|_| "\"\"".to_string(), |ident| ident.quote(Dialect::SQLite));
                let count = pool
                    .fetch_optional(&Stmt::new(
                        format!("SELECT COUNT(*) AS rows FROM {quoted}"),
                        vec![],
                    ))
                    .await
                    .ok()
                    .flatten()
                    .and_then(|row| row.i64("rows").ok())
                    .unwrap_or(0);
                let bytes = pool
                    .fetch_optional(&Stmt::new(
                        "SELECT COALESCE(SUM(pgsize), 0) AS bytes FROM dbstat WHERE name = ?",
                        vec![Value::from(name.as_str())],
                    ))
                    .await
                    .ok()
                    .flatten()
                    .and_then(|row| row.i64("bytes").ok())
                    .unwrap_or(0);
                usage.push(TableUsage {
                    name,
                    rows: count,
                    // `bytes` is a page count times page size; the fractional
                    // megabytes are what the report shows, and the loss of
                    // precision is far below a page.
                    #[allow(clippy::cast_precision_loss)]
                    size_mb: bytes as f64 / 1_048_576.0,
                });
            }
            Ok(usage)
        }
    }
}

/// Mirror the queue's durable job rows onto `RQ Job`.
#[cfg(feature = "jobs")]
async fn write_rq_jobs(queue: &JobQueue, pool: &Db, mirrors: &mut Mirrors) {
    match queue.list_jobs(None, None, 200).await {
        Ok(jobs) => {
            for job in jobs {
                let status = match job.status {
                    JobStatus::Pending | JobStatus::Queued | JobStatus::Retrying => "Queued",
                    JobStatus::Running => "Started",
                    JobStatus::Success => "Finished",
                    JobStatus::Failed | JobStatus::Cancelled | JobStatus::DeadLetter => "Failed",
                };
                let completed = job
                    .result
                    .as_ref()
                    .map(|result| result.completed_at.to_rfc3339());
                let error = job.result.as_ref().and_then(|result| result.error.clone());
                upsert(
                    pool,
                    mirrors,
                    format!("RQ Job:{}", job.id),
                    "RQ Job",
                    serde_json::json!({
                        "job_id": job.id,
                        "queue": job.queue,
                        "status": status,
                        "started_at": job.started_at.map(|at| at.to_rfc3339()),
                        "ended_at": completed,
                        "result": job.result.as_ref().and_then(|r| r.output.clone()),
                        "exc_info": error,
                    }),
                )
                .await;
            }
        }
        Err(error) => debug!(%error, "job list failed"),
    }
}

/// Mirror live worker heartbeats onto `RQ Worker`.
#[cfg(feature = "jobs")]
async fn write_rq_workers(queue: &JobQueue, pool: &Db, mirrors: &mut Mirrors) {
    match queue.list_workers().await {
        Ok(workers) => {
            for worker in workers {
                let queue_name = worker.queues.first().cloned().unwrap_or_default();
                let status = if worker.status.eq_ignore_ascii_case("running") {
                    if worker.current_jobs > 0 {
                        "Busy"
                    } else {
                        "Idle"
                    }
                } else {
                    "Stopped"
                };
                upsert(
                    pool,
                    mirrors,
                    format!("RQ Worker:{}", worker.worker_id),
                    "RQ Worker",
                    serde_json::json!({
                        "worker_name": worker.worker_id,
                        "queue": queue_name,
                        "status": status,
                        "last_heartbeat": DateTime::from_timestamp(worker.last_heartbeat, 0)
                            .map(|at| at.to_rfc3339()),
                    }),
                )
                .await;
            }
        }
        Err(error) => debug!(%error, "worker list failed"),
    }
}

/// Mirror scheduler definitions onto `Scheduled Job Type`, and a `Scheduled Job
/// Log` row whenever a schedule's `run_count` advances.
///
/// The scheduler keeps no execution history of its own, so the log records the
/// last run it reports (`last_run`, `last_error`) at the moment the count
/// changes. A run that starts and fails between two snapshots still surfaces
/// because both counters are visible on the next tick.
#[cfg(feature = "scheduler")]
async fn write_schedules(scheduler: &Scheduler, pool: &Db, mirrors: &mut Mirrors) {
    let listed = match scheduler.list_schedules().await {
        Ok(rows) => rows,
        Err(error) => {
            debug!(%error, "schedule list failed");
            return;
        }
    };
    for definition in listed {
        upsert(
            pool,
            mirrors,
            format!("Scheduled Job Type:{}", definition.id),
            "Scheduled Job Type",
            serde_json::json!({
                "name": definition.name.clone(),
                "cron_format": definition.config.cron_expression,
                "last_execution": definition.last_run.map(|at| at.to_rfc3339()),
                "next_execution": definition.next_run.map(|at| at.to_rfc3339()),
                "disabled": !definition.enabled,
                "stopped": false,
            }),
        )
        .await;

        let previous = mirrors
            .schedule_runs
            .get(&definition.id)
            .copied()
            .unwrap_or(0);
        if definition.run_count > previous && definition.last_run.is_some() {
            let status = if definition.last_error.is_some() {
                "Failed"
            } else {
                "Finished"
            };
            let at = definition.last_run.map(|at| at.to_rfc3339());
            write_row(
                pool,
                "Scheduled Job Log",
                serde_json::json!({
                    // The `Scheduled Job Type` document is created with
                    // `name = definition.name`, so the link stores the same
                    // value rather than the scheduler's internal id.
                    "job_type": definition.name,
                    "status": status,
                    "start_time": at,
                    "end_time": at,
                    "output": format!("run_count={}", definition.run_count),
                    "error": definition.last_error,
                }),
            )
            .await;
        }
        mirrors
            .schedule_runs
            .insert(definition.id, definition.run_count);
    }
}
