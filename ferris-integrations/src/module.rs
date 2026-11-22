//! The integrations feature as a declarative native module.
//!
//! The DocTypes and the Integrations workspace are owned by this crate; the
//! runtime (webhook dispatch, automation, connectors) is installed by
//! `ferris-server`, which owns the shared job queue and the mailer port.

use ferris_api::Module;
use ferris_core::workspaces::WorkspaceRegistration;

/// DocTypes and the integrations workspace.
#[derive(Debug, Default, Clone, Copy)]
pub struct IntegrationsModule;

impl Module for IntegrationsModule {
    fn name(&self) -> &'static str {
        "integrations"
    }

    fn register_doctypes(&self) {
        crate::doctypes::register_doctypes();
    }

    fn register_workspaces(&self) -> WorkspaceRegistration {
        crate::workspaces::register_workspaces()
    }
}
