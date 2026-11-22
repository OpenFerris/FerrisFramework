//! The `Arc<Scheduler>` facade handed to the server and the WASM host.

use super::Scheduler;
use crate::schedule::Schedule;
use ferris_core::Result;
use std::sync::Arc;

pub struct SchedulerHandle {
    scheduler: Arc<Scheduler>,
}

impl SchedulerHandle {
    pub fn new(scheduler: Arc<Scheduler>) -> Self {
        Self { scheduler }
    }

    /// Add a new schedule
    pub async fn add_schedule(&self, schedule: Schedule) -> Result<String> {
        self.scheduler.add_schedule(schedule).await
    }

    /// Remove a schedule
    pub async fn remove_schedule(&self, schedule_id: &str) -> Result<()> {
        self.scheduler.remove_schedule(schedule_id).await
    }

    /// Enable a schedule
    pub async fn enable_schedule(&self, schedule_id: &str) -> Result<()> {
        self.scheduler.enable_schedule(schedule_id).await
    }

    /// Disable a schedule
    pub async fn disable_schedule(&self, schedule_id: &str) -> Result<()> {
        self.scheduler.disable_schedule(schedule_id).await
    }

    /// Trigger a schedule immediately
    pub async fn trigger_now(&self, schedule_id: &str) -> Result<()> {
        self.scheduler.trigger_now(schedule_id).await
    }

    /// Get schedule by ID
    pub async fn get_schedule(&self, schedule_id: &str) -> Result<Option<Schedule>> {
        self.scheduler.get_schedule(schedule_id).await
    }

    /// List all schedules
    pub async fn list_schedules(&self) -> Result<Vec<Schedule>> {
        self.scheduler.list_schedules().await
    }

    /// Shutdown scheduler
    pub async fn shutdown(&self) -> Result<()> {
        self.scheduler.shutdown().await
    }
}
