//! `scheduler` interface of [`HostApi`](crate::host::HostApi).

use super::RealHostApi;
use crate::host::ScheduleInfo;
use anyhow::Result;
use ferris_core::Value;
use ferris_jobs::{Job, JobPriority};
use std::sync::Arc;

impl RealHostApi {
    /// The cron scheduler, created once, **started once**, and only when a job
    /// queue is reachable.
    ///
    /// A `Scheduler` that is built but never started is a table nobody reads:
    /// `platform::schedule` used to persist a row, return `Ok`, and log a
    /// warning, so the guest believed it had a cron while no engine ever fired
    /// it (SCHED-03/SCHED-10). The scheduler is now started before it accepts
    /// the first schedule, and a build/start failure is an error the guest sees.
    ///
    /// # Errors
    ///
    /// Fails when the backend exists but the scheduler cannot be built or
    /// started.
    async fn scheduler(&self) -> Result<Option<Arc<ferris_scheduler::Scheduler>>> {
        // No `parking_lot` guard is held across the `.await` below (its guard
        // is not `Send`, so the future would never leave the current thread
        // and the scheduler call could not run on a worker): read once, drop
        // the guard, build off-lock, then store under a fresh guard only when
        // nobody else won the race while we were building.
        if let Some(scheduler) = self.scheduler.lock().as_ref() {
            return Ok(Some(scheduler.clone()));
        }
        let Some(queue) = &self.job_queue else {
            return Ok(None);
        };
        let built = ferris_scheduler::Scheduler::new(queue.clone(), self.pool.clone())
            .await
            .map_err(|e| anyhow::anyhow!("failed to build the cron scheduler: {e}"))?;

        // Publish first, then start, so a concurrent caller reuses this
        // instance instead of building a second engine that would enqueue a
        // duplicate copy of every run. Only the caller that inserted it starts
        // it; the loser sees an instance the winner is about to start.
        //
        // The guard is dropped inside the closure deliberately: a
        // `parking_lot` guard is not `Send`, so it must not be reachable from
        // the `.await` below (the `HostApi` futures are boxed as `Send`).
        let (scheduler, fresh) = self.scheduler.lock().clone().map_or_else(
            || {
                let candidate = Arc::new(built);
                let mut guard = self.scheduler.lock();
                let winner = guard.clone();
                let fresh = winner.is_none();
                if fresh {
                    *guard = Some(candidate.clone());
                }
                drop(guard);
                (winner.unwrap_or(candidate), fresh)
            },
            |existing| (existing, false),
        );
        if fresh && let Err(e) = scheduler.start().await {
            // Never keep a scheduler that failed to start: the next call must
            // retry instead of adding schedules to a dead engine.
            let mut guard = self.scheduler.lock();
            if guard
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &scheduler))
            {
                *guard = None;
            }
            drop(guard);
            return Err(anyhow::anyhow!("failed to start the cron scheduler: {e}"));
        }
        Ok(Some(scheduler))
    }
}

impl RealHostApi {
    // -- scheduler ----------------------------------------------------------

    pub(super) async fn scheduler_schedule(&self, plugin: &str, record: &str) -> Result<String> {
        // A `ScheduledJob` payload: a cron expression, the function to run and
        // its arguments. Registered with the cron scheduler, not queued now.
        let value: Value = serde_json::from_str(record)?;
        let cron = value
            .get("cron")
            .and_then(|v| v.as_str())
            .filter(|c| !c.is_empty())
            .ok_or_else(|| anyhow::anyhow!("schedule payload needs a `cron` expression"))?;
        let job_name = value
            .get("job_name")
            .and_then(|v| v.as_str())
            .unwrap_or("scheduled_job");
        // A schedule may only aim at its own app. An unprefixed name stays
        // allowed (single-app installs have used those since the first
        // scaffold), but another app's `<other>.<job>` is refused here rather
        // than enqueued into a job nobody owns.
        if let Some((prefix, rest)) = job_name.split_once('.')
            && !rest.is_empty()
            && prefix != plugin
        {
            anyhow::bail!(
                "app `{plugin}` may not schedule job `{job_name}`: `{prefix}` is not this app"
            );
        }

        let mut schedule = ferris_scheduler::Schedule::new(
            job_name.to_string(),
            ferris_scheduler::ScheduleConfig::default(),
        )
        .with_cron(cron.to_string())
        .with_job_name(job_name.to_string());

        if let Some(payload) = value.get("payload") {
            schedule = schedule.with_payload(payload.clone());
        }
        if let Some(queue) = value.get("queue").and_then(|v| v.as_str()) {
            schedule = schedule.with_queue(queue.to_string());
        }
        if let Some(priority) = value.get("priority").and_then(|v| v.as_str())
            && let Some(p) = parse_priority(priority)
        {
            schedule = schedule.with_priority(p);
        }
        // The schedule table is installation-wide, so ownership is carried on
        // the schedule itself: `list`/`cancel` only ever see tags of the
        // calling plugin. Ops' own tags from the payload are preserved.
        let mut tags: Vec<String> = value
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|tags| {
                tags.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        tags.push(plugin_tag(plugin));
        schedule = schedule.with_tags(tags);

        let schedule_id = schedule.id.clone();
        // No backend is a refusal, not a silent success: a guest that gets `Ok`
        // believes its cron exists, and it never will (SCHED-03/SCHED-10).
        let Some(scheduler) = self.scheduler().await? else {
            anyhow::bail!(
                "no scheduler backend: `schedule` needs a job queue (redis or the `jobs` table)"
            );
        };
        scheduler
            .add_schedule(schedule)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(schedule_id)
    }

    pub(super) async fn scheduler_cancel(&self, plugin: &str, schedule_id: &str) -> Result<()> {
        let Some(scheduler) = self.scheduler().await? else {
            // No scheduler backend: nothing was ever registered, so there is
            // nothing to cancel. Idempotent by contract.
            return Ok(());
        };
        let owned = scheduler
            .list_schedules()
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        match owned.iter().find(|s| s.id == schedule_id) {
            Some(schedule) if owns_schedule(schedule, plugin) => {
                scheduler
                    .remove_schedule(schedule_id)
                    .await
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                Ok(())
            }
            // Already gone: cancel is idempotent.
            None => Ok(()),
            // Someone else's schedule: refuse rather than silently no-op.
            Some(_) => anyhow::bail!("schedule `{schedule_id}` belongs to another app"),
        }
    }

    pub(super) async fn scheduler_list(&self, plugin: &str) -> Result<Vec<ScheduleInfo>> {
        let Some(scheduler) = self.scheduler().await? else {
            return Ok(Vec::new());
        };
        let owned = scheduler
            .list_schedules()
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        Ok(owned
            .iter()
            .filter(|schedule| owns_schedule(schedule, plugin))
            .map(|schedule| ScheduleInfo {
                id: schedule.id.clone(),
                job_name: schedule.config.job_name.clone(),
                cron: schedule.config.cron_expression.clone(),
                enabled: schedule.enabled,
            })
            .collect())
    }

    /// Whether `job_type` may be enqueued by `plugin`.
    ///
    /// A job type belongs to an app when it is named `<app>.<job>`, the shape
    /// `platform::schedule` documents, or when it names no app at all and only
    /// one app is installed (a lone app cannot confuse itself with another).
    /// Without this an app could enqueue any type, including a framework job or
    /// another app's, and the worker would offer it to that app
    /// (WASM-007/SEC-11).
    #[must_use]
    fn owns_job_type(plugin: &str, job_type: &str) -> bool {
        match job_type.split_once('.') {
            Some((prefix, rest)) => !rest.is_empty() && prefix == plugin,
            None => true,
        }
    }

    /// Enqueue one job **as `plugin`**, refusing a type that app does not own.
    ///
    /// # Errors
    ///
    /// Fails for a foreign or malformed job type, and when the queue rejects the
    /// job.
    pub(super) async fn scheduler_enqueue_for(
        &self,
        plugin: &str,
        job_type: &str,
        payload: &str,
    ) -> Result<()> {
        if job_type.trim().is_empty() || !Self::owns_job_type(plugin, job_type.trim()) {
            anyhow::bail!(
                "app `{plugin}` may not enqueue job type `{job_type}`; \
                 name app jobs `<app>.<job>` (`{plugin}.<job>`)"
            );
        }
        let payload_value: Value =
            serde_json::from_str(payload).unwrap_or_else(|_| Value::String(payload.to_string()));
        let job = Job::new(job_type.to_string(), payload_value);
        match &self.job_queue {
            Some(queue) => {
                queue
                    .enqueue(job)
                    .await
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            }
            None => {
                tracing::warn!(
                    job_type,
                    "job accepted but not enqueued: no job queue backend (redis?)"
                );
            }
        }
        Ok(())
    }
}

/// Ownership tag the host stamps on a plugin's cron schedules. The schedule
/// table is installation-wide, so the tag is how `list-schedules`/`cancel`
/// scope to the calling plugin.
fn plugin_tag(plugin: &str) -> String {
    format!("plugin:{plugin}")
}

/// Whether `schedule` was registered by `plugin`.
fn owns_schedule(schedule: &ferris_scheduler::Schedule, plugin: &str) -> bool {
    let tag = plugin_tag(plugin);
    schedule.config.tags.contains(&tag)
}

fn parse_priority(value: &str) -> Option<JobPriority> {
    match value {
        "low" => Some(JobPriority::Low),
        "normal" => Some(JobPriority::Normal),
        "high" => Some(JobPriority::High),
        "critical" => Some(JobPriority::Critical),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::RealHostApi;

    /// WASM-007/SEC-11: an app may only enqueue `<app>.<job>` (or a bare type,
    /// which belongs to a lone install).
    #[test]
    fn an_app_owns_only_its_own_job_type_prefix() {
        assert!(RealHostApi::owns_job_type("a", "a.nightly"));
        assert!(RealHostApi::owns_job_type("a", "task_nightly"));
        assert!(!RealHostApi::owns_job_type("a", "b.nightly"));
        assert!(!RealHostApi::owns_job_type("a", "a."));
        assert!(!RealHostApi::owns_job_type("a", "framework.sweep"));
    }

    async fn host() -> RealHostApi {
        let db = ferris_db::memory_db().await.expect("memory db");
        RealHostApi::new(db)
    }

    /// SCHED-03/SCHED-10: without a backend `schedule` must fail rather than
    /// persist a row nothing will ever fire.
    #[tokio::test]
    async fn schedule_without_a_backend_is_an_error() {
        let mut host = host().await;
        host.job_queue = None;
        let err = host
            .scheduler_schedule("a", r#"{"cron":"0 3 * * *","job_name":"a.monthly"}"#)
            .await
            .expect_err("no scheduler backend must be a refusal");
        assert!(
            err.to_string().contains("no scheduler backend"),
            "unexpected error: {err}"
        );
    }

    /// A schedule may not aim at another app's job type.
    #[tokio::test]
    async fn schedule_cannot_target_another_app() {
        let host = host().await;
        let err = host
            .scheduler_schedule("a", r#"{"cron":"0 3 * * *","job_name":"b.monthly"}"#)
            .await
            .expect_err("another app's job must be refused");
        assert!(err.to_string().contains("is not this app"), "{err}");
    }

    /// The scoped enqueue rejects a foreign type before it reaches the queue.
    #[tokio::test]
    async fn enqueue_job_of_a_foreign_type_is_refused() {
        let host = host().await;
        let err = host
            .scheduler_enqueue_for("a", "b.nightly", "{}")
            .await
            .expect_err("a foreign job type must be refused");
        assert!(err.to_string().contains("may not enqueue"), "{err}");
        // The app's own type passes the ownership check (the queue may still
        // report that it has no backend, which is not a refusal).
        let _ = host.scheduler_enqueue_for("a", "a.nightly", "{}").await;
    }
}
