//! `platform::process-*` of [`HostApi`](crate::host::HostApi), forwarded to the
//! running server's supervisor.
//!
//! A factory is a child of the **host**: forking, wiring the pipes, enforcing
//! the deadline and reaping the tree are the server's job, and the process has
//! to outlive a guest call (a spawned program keeps running until
//! `process-wait` or `process-kill`). This runtime owns no reactor for it, so
//! it forwards to the process-wide [`ProcessService`](crate::host::ProcessService)
//! and answers `unsupported` when none is registered — the honest failure the
//! WIT `platform::features` surface promises.

use super::RealHostApi;
use crate::host::{ProcessOutput, ProcessSpec, process_service};
use anyhow::Result;

impl RealHostApi {
    pub(super) async fn process_run(&self, app: &str, spec: &ProcessSpec) -> Result<ProcessOutput> {
        match process_service() {
            Some(service) => service.run(app, spec).await,
            None => Err(crate::host::BridgeError::unsupported(
                "platform::process-run (no process supervisor registered in this process)",
            )),
        }
    }

    pub(super) async fn process_spawn(&self, app: &str, spec: &ProcessSpec) -> Result<String> {
        match process_service() {
            Some(service) => service.spawn(app, spec).await,
            None => Err(crate::host::BridgeError::unsupported(
                "platform::process-spawn (no process supervisor registered in this process)",
            )),
        }
    }

    pub(super) async fn process_wait(&self, app: &str, process_id: &str) -> Result<ProcessOutput> {
        match process_service() {
            Some(service) => service.wait(app, process_id).await,
            None => Err(crate::host::BridgeError::unsupported(
                "platform::process-wait (no process supervisor registered in this process)",
            )),
        }
    }

    pub(super) async fn process_kill(&self, app: &str, process_id: &str) -> Result<()> {
        match process_service() {
            Some(service) => service.kill(app, process_id).await,
            None => Err(crate::host::BridgeError::unsupported(
                "platform::process-kill (no process supervisor registered in this process)",
            )),
        }
    }
}
