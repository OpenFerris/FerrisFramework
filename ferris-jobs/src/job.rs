//! Job definitions and core types

use chrono::{DateTime, Utc};
use ferris_core::{Result, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

/// Job priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobPriority {
    Low = -10,
    Normal = 0,
    High = 10,
    Critical = 20,
}

impl Default for JobPriority {
    fn default() -> Self {
        JobPriority::Normal
    }
}

/// Job status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Queued,
    Running,
    Success,
    Failed,
    Retrying,
    Cancelled,
    DeadLetter,
}

/// Job result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResult {
    pub success: bool,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub duration_ms: u64,
}

impl JobStatus {
    /// Stable snake_case representation for persistence queries.
    pub fn as_db_str(&self) -> &'static str {
        match self {
            JobStatus::Pending => "pending",
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Success => "success",
            JobStatus::Failed => "failed",
            JobStatus::Retrying => "retrying",
            JobStatus::Cancelled => "cancelled",
            JobStatus::DeadLetter => "dead_letter",
        }
    }
}

impl JobResult {
    pub fn success(output: Value) -> Self {
        let now = Utc::now();
        Self {
            success: true,
            output: Some(output),
            error: None,
            started_at: now,
            completed_at: now,
            duration_ms: 0,
        }
    }

    pub fn failure(error: String) -> Self {
        let now = Utc::now();
        Self {
            success: false,
            output: None,
            error: Some(error),
            started_at: now,
            completed_at: now,
            duration_ms: 0,
        }
    }
}

/// Job definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub queue: String,
    pub payload: Value,
    pub priority: JobPriority,
    pub status: JobStatus,
    pub retries: u32,
    pub max_retries: u32,
    pub timeout: Duration,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub depends_on: Vec<String>, // Job IDs this job depends on
    pub tags: Vec<String>,
    pub metadata: std::collections::HashMap<String, Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub worker_id: Option<String>,
    pub result: Option<JobResult>,
}

impl Job {
    pub fn new(name: String, payload: Value) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            queue: "default".to_string(),
            payload,
            priority: JobPriority::Normal,
            status: JobStatus::Pending,
            retries: 0,
            max_retries: 3,
            timeout: Duration::from_secs(300),
            scheduled_at: None,
            started_at: None,
            completed_at: None,
            depends_on: Vec::new(),
            tags: Vec::new(),
            metadata: HashMap::new(),
            created_at: now,
            updated_at: now,
            worker_id: None,
            result: None,
        }
    }

    pub fn with_queue(mut self, queue: String) -> Self {
        self.queue = queue;
        self
    }

    pub fn with_priority(mut self, priority: JobPriority) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_scheduled_at(mut self, scheduled_at: DateTime<Utc>) -> Self {
        self.scheduled_at = Some(scheduled_at);
        self
    }

    pub fn with_depends_on(mut self, depends_on: Vec<String>) -> Self {
        self.depends_on = depends_on;
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn with_metadata(mut self, key: String, value: Value) -> Self {
        self.metadata.insert(key, value);
        self
    }

    pub fn is_ready(&self) -> bool {
        self.status == JobStatus::Pending
            || self.status == JobStatus::Queued
            || self.status == JobStatus::Retrying
    }

    pub fn can_retry(&self) -> bool {
        self.retries < self.max_retries
            && (self.status == JobStatus::Failed || self.status == JobStatus::Retrying)
    }

    pub fn is_due(&self) -> bool {
        if let Some(scheduled) = self.scheduled_at {
            Utc::now() >= scheduled
        } else {
            true
        }
    }
}

/// Job handler trait
#[async_trait::async_trait]
pub trait JobHandler: Send + Sync {
    type Payload: Serialize + for<'de> Deserialize<'de> + Send + Sync;
    type Output: Serialize + Send + Sync;

    fn job_name(&self) -> &'static str;
    fn queue(&self) -> &str {
        "default"
    }
    fn priority(&self) -> JobPriority {
        JobPriority::Normal
    }
    fn max_retries(&self) -> u32 {
        3
    }
    fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(300)
    }

    async fn handle(&self, payload: Self::Payload, ctx: JobContext) -> Result<Self::Output>;
}

/// Job execution context
#[derive(Debug, Clone)]
pub struct JobContext {
    pub job_id: String,
    pub job_name: String,
    pub queue: String,
    pub attempt: u32,
    pub tenant_id: Option<String>,
    pub user_id: Option<String>,
    pub metadata: std::collections::HashMap<String, ferris_core::Value>,
}

impl JobContext {
    pub fn new(job_id: String, job_name: String, queue: String) -> Self {
        Self {
            job_id,
            job_name,
            queue,
            attempt: 1,
            tenant_id: None,
            user_id: None,
            metadata: HashMap::new(),
        }
    }

    pub fn with_attempt(mut self, attempt: u32) -> Self {
        self.attempt = attempt;
        self
    }

    pub fn with_tenant(mut self, tenant_id: String) -> Self {
        self.tenant_id = Some(tenant_id);
        self
    }

    pub fn with_user(mut self, user_id: String) -> Self {
        self.user_id = Some(user_id);
        self
    }

    pub fn with_metadata(mut self, key: String, value: ferris_core::Value) -> Self {
        self.metadata.insert(key, value);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_creation() {
        let job = Job::new("test_job".to_string(), ferris_core::Value::from("test"));

        assert!(!job.id.is_empty());
        assert_eq!(job.name, "test_job");
        assert_eq!(job.status, JobStatus::Pending);
        assert_eq!(job.max_retries, 3);
    }

    #[test]
    fn test_job_builder() {
        let job = Job::new("test".to_string(), ferris_core::Value::from("data"))
            .with_queue("high".to_string())
            .with_priority(JobPriority::High)
            .with_max_retries(5)
            .with_timeout(std::time::Duration::from_secs(600))
            .with_tags(vec!["important".to_string()]);

        assert_eq!(job.queue, "high");
        assert_eq!(job.priority, JobPriority::High);
        assert_eq!(job.max_retries, 5);
        assert_eq!(job.timeout, std::time::Duration::from_secs(600));
        assert_eq!(job.tags, vec!["important"]);
    }

    #[test]
    fn test_job_status() {
        let mut job = Job::new("test".to_string(), ferris_core::Value::Null);

        assert!(job.is_ready());
        assert!(!job.can_retry());
        assert!(job.is_due());

        job.status = JobStatus::Failed;
        assert!(job.can_retry());

        job.retries = job.max_retries;
        assert!(!job.can_retry());
    }
}
