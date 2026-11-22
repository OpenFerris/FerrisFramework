//! The email feature as a declarative native module.
//!
//! DocTypes and the Desk workspace used to be registered by
//! `ferris-server::catalog`; they now travel with the crate that owns them. The
//! transport (SMTP accounts, queue and worker) stays in `ferris-server` because
//! it shares the job queue with the rest of the host.

use ferris_api::Module;
use ferris_core::workspaces::WorkspaceRegistration;

/// DocTypes and the email workspace.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmailModule;

impl Module for EmailModule {
    fn name(&self) -> &'static str {
        "email"
    }

    fn register_doctypes(&self) {
        crate::doctypes::register_doctypes();
    }

    fn register_workspaces(&self) -> WorkspaceRegistration {
        crate::workspaces::register_workspaces()
    }
}
