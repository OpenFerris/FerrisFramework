use crate::job::{JobContext, JobHandler, JobPriority};
use async_trait::async_trait;
use ferris_core::{Error, Result, ValidationError, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// A boxed JobHandler that can be stored in a HashMap
type BoxedHandler = Arc<dyn JobHandler<Payload = Value, Output = Value> + Send + Sync>;

/// Job dispatcher that routes jobs to registered handlers based on job name
#[derive(Default)]
pub struct JobDispatcher {
    handlers: HashMap<String, BoxedHandler>,
    /// Handler tried when no exact-name handler matches the claimed job's
    /// `job_name`. The server installs the plugin bridge here so a plugin job
    /// reaches the guest's `handle-job` export; exact-name handlers (the
    /// integrations' sweeps) always win.
    fallback: Option<BoxedHandler>,
}

impl JobDispatcher {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            fallback: None,
        }
    }

    /// Register a handler for a specific job name
    pub fn register<H>(&mut self, handler: H)
    where
        H: JobHandler<Payload = Value, Output = Value> + Send + Sync + 'static,
    {
        let name = handler.job_name().to_string();
        self.handlers.insert(name, Arc::new(handler));
    }

    /// Register the handler tried when no exact-name handler matches, so jobs
    /// the dispatcher does not otherwise know (e.g. a scheduled plugin job)
    /// are still offered to the app instead of failing as "no handler".
    ///
    /// The exact-name handlers registered with [`register`](Self::register)
    /// take precedence; the fallback only fills the gap.
    pub fn set_fallback<H>(&mut self, handler: H)
    where
        H: JobHandler<Payload = Value, Output = Value> + Send + Sync + 'static,
    {
        self.fallback = Some(Arc::new(handler));
    }

    /// Get a handler by job name
    pub fn get_handler(&self, job_name: &str) -> Option<&BoxedHandler> {
        self.handlers.get(job_name)
    }

    /// Check if a handler is registered for a job name
    pub fn has_handler(&self, job_name: &str) -> bool {
        self.handlers.contains_key(job_name)
    }

    /// Get all registered job names
    pub fn job_names(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }
}

#[async_trait]
impl JobHandler for JobDispatcher {
    type Payload = Value;
    type Output = Value;

    fn job_name(&self) -> &'static str {
        "dispatcher"
    }

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

    async fn handle(&self, payload: Value, ctx: JobContext) -> Result<Value> {
        // Extract the actual job name from the payload or context
        // The job name is in ctx.job_name
        let handler = self
            .get_handler(&ctx.job_name)
            .or(self.fallback.as_ref())
            .ok_or_else(|| {
                Error::Validation(ValidationError::Invalid(format!(
                    "No handler registered for job: {}",
                    ctx.job_name
                )))
            })?;

        handler.handle(payload, ctx).await
    }
}
