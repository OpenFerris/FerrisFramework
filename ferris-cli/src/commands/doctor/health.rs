//! The value every check returns.

#[derive(Debug)]
pub(super) struct HealthCheck {
    pub(super) name: String,
    pub(super) passed: bool,
    pub(super) message: String,
    pub(super) critical: bool,
}
