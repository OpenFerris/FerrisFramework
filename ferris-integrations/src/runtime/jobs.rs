//! Job handlers for the scheduled integrations.
//!
//! Each handler runs one [`run_due`](auto_repeat::run_due)-style sweep; the
//! host registers them on its `JobDispatcher` and enqueues a job per tick (or
//! points a scheduler event at each [`job_names`]). A host that prefers to
//! drive the sweeps directly can call the `run_due` functions instead — the
//! handlers are a thin bridge.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::Utc;
use ferris_core::{Result, Value};
use ferris_jobs::{JobContext, JobDispatcher, JobHandler};

use super::auto_email_report;
use super::auto_repeat;
use super::push;
use super::reminder;
use super::state::IntegrationsContext;

/// A sweep's row count as a `Value`, saturating the `usize` into `i64`.
fn count_value(count: usize) -> Value {
    Value::from(i64::try_from(count).unwrap_or(i64::MAX))
}

/// The job name a scheduler enqueues for the daily auto-repeat sweep.
pub const JOB_AUTO_REPEAT: &str = "integrations_auto_repeat";
/// The job name a scheduler enqueues for the reminder sweep.
pub const JOB_REMINDER: &str = "integrations_reminder";
/// The job name a scheduler enqueues for the auto-email-report sweep.
pub const JOB_AUTO_EMAIL_REPORT: &str = "integrations_auto_email_report";
/// The job name a scheduler enqueues for pending push delivery.
pub const JOB_PUSH_DELIVERY: &str = "integrations_push_delivery";

/// The `job_name`s a scheduler should enqueue under.
#[must_use]
pub const fn job_names() -> &'static [&'static str] {
    &[
        JOB_AUTO_REPEAT,
        JOB_REMINDER,
        JOB_AUTO_EMAIL_REPORT,
        JOB_PUSH_DELIVERY,
    ]
}

/// Run the `Auto Repeat` due sweep (daily cadence).
#[derive(Clone)]
pub struct AutoRepeatJob {
    pub state: IntegrationsContext,
}

#[async_trait]
impl JobHandler for AutoRepeatJob {
    type Payload = Value;
    type Output = Value;

    fn job_name(&self) -> &'static str {
        JOB_AUTO_REPEAT
    }

    async fn handle(&self, _payload: Value, _ctx: JobContext) -> Result<Value> {
        let created = auto_repeat::run_due(&self.state, Utc::now().date_naive()).await?;
        Ok(Value::Object(HashMap::from([(
            "created".to_string(),
            count_value(created),
        )])))
    }
}

/// Run the `Reminder` due sweep (minute cadence).
#[derive(Clone)]
pub struct ReminderJob {
    pub state: IntegrationsContext,
}

#[async_trait]
impl JobHandler for ReminderJob {
    type Payload = Value;
    type Output = Value;

    fn job_name(&self) -> &'static str {
        JOB_REMINDER
    }

    async fn handle(&self, _payload: Value, _ctx: JobContext) -> Result<Value> {
        let delivered = reminder::run_due(&self.state, Utc::now()).await?;
        Ok(Value::Object(HashMap::from([(
            "delivered".to_string(),
            count_value(delivered),
        )])))
    }
}

/// Run the `Auto Email Report` due sweep (daily cadence).
#[derive(Clone)]
pub struct AutoEmailReportJob {
    pub state: IntegrationsContext,
}

#[async_trait]
impl JobHandler for AutoEmailReportJob {
    type Payload = Value;
    type Output = Value;

    fn job_name(&self) -> &'static str {
        JOB_AUTO_EMAIL_REPORT
    }

    async fn handle(&self, _payload: Value, _ctx: JobContext) -> Result<Value> {
        let sent = auto_email_report::run_due(&self.state, Utc::now()).await?;
        Ok(Value::Object(HashMap::from([(
            "sent".to_string(),
            count_value(sent),
        )])))
    }
}

/// Deliver every `Push Notification` row still `Queued`.
#[derive(Clone)]
pub struct PushDeliveryJob {
    pub state: IntegrationsContext,
}

#[async_trait]
impl JobHandler for PushDeliveryJob {
    type Payload = Value;
    type Output = Value;

    fn job_name(&self) -> &'static str {
        JOB_PUSH_DELIVERY
    }

    async fn handle(&self, _payload: Value, _ctx: JobContext) -> Result<Value> {
        let delivered = push::deliver_pending(&self.state).await?;
        Ok(Value::Object(HashMap::from([(
            "delivered".to_string(),
            count_value(delivered),
        )])))
    }
}

/// Register every scheduled integrations handler on `dispatcher`.
pub fn register_handlers(dispatcher: &mut JobDispatcher, state: IntegrationsContext) {
    dispatcher.register(AutoRepeatJob {
        state: state.clone(),
    });
    dispatcher.register(ReminderJob {
        state: state.clone(),
    });
    dispatcher.register(AutoEmailReportJob {
        state: state.clone(),
    });
    dispatcher.register(PushDeliveryJob { state });
}
