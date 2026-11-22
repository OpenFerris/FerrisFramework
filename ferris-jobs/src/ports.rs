//! The `ferris_core::ports::QueuePort` adapter for the real job queue.

use std::sync::Arc;

use ferris_core::error::JobError;
use ferris_core::ports::{JobRequest, QueuePort};
use ferris_core::{Error, Result};
use tokio::runtime::Handle;

use crate::{Job, JobQueue};

/// Enqueue through the host's durable queue without exposing its concrete type.
#[derive(Clone)]
pub struct JobQueuePort {
    queue: Arc<JobQueue>,
    handle: Handle,
}

impl JobQueuePort {
    /// # Panics
    ///
    /// Panics when called outside a Tokio runtime: the adapter enqueues by
    /// spawning, and a caller without a runtime has nowhere to put the future.
    /// Use [`Self::try_new`] when the caller cannot guarantee a runtime.
    #[must_use]
    pub fn new(queue: Arc<JobQueue>) -> Self {
        Self {
            queue,
            handle: Handle::current(),
        }
    }

    /// Build the adapter, or say why the current thread has no runtime.
    ///
    /// # Errors
    ///
    /// [`Error::Job`] when called outside a Tokio runtime.
    // `ferris_core::Error` is the shared taxonomy; see `QueuePort`.
    #[allow(clippy::result_large_err)]
    pub fn try_new(queue: Arc<JobQueue>) -> Result<Self> {
        let handle = Handle::try_current().map_err(|error| {
            Error::Job(JobError::Failed(format!(
                "job queue port needs a Tokio runtime: {error}"
            )))
        })?;
        Ok(Self { queue, handle })
    }
}

impl QueuePort for JobQueuePort {
    fn enqueue(&self, request: &JobRequest) -> Result<()> {
        let mut job = Job::new(request.job_type.clone(), request.payload.clone());
        if let Some(queue) = &request.queue {
            job.queue.clone_from(queue);
        }
        let queue = self.queue.clone();
        self.handle.spawn(async move {
            if let Err(error) = queue.enqueue(job).await {
                tracing::error!(%error, "job queue port: enqueue failed");
            }
        });
        Ok(())
    }
}
