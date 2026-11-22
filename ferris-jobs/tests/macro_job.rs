//! Integration test for the `#[job]` macro.
//!
//! The macro used to expand to `#input` and drop its arguments, so a job
//! declared with `queue = "billing"` was indistinguishable from a plain struct.
//! It must now build a `Job` carrying every declared knob and the sensible
//! defaults for the ones left out.

// Integration tests link the crate's whole dependency set but reference only a
// handful; the workspace's `unused_crate_dependencies` lint would otherwise fire
// once per unreferenced dependency.
#![allow(unused_crate_dependencies)]

use ferris_macro::job;

#[job(
    name = "send_invoice",
    queue = "billing",
    priority = 10,
    max_retries = 5,
    timeout_secs = 120
)]
#[derive(Debug, Clone)]
pub struct SendInvoiceJob {}

#[job]
#[derive(Debug, Clone)]
pub struct DefaultsJob {}

#[test]
fn job_macro_builds_a_configured_job() {
    let job = SendInvoiceJob::job(ferris_core::Value::from("inv-1"));
    assert_eq!(job.name, "send_invoice");
    assert_eq!(job.queue, "billing");
    assert_eq!(job.priority, ferris_jobs::JobPriority::High);
    assert_eq!(job.max_retries, 5);
    assert_eq!(job.timeout, std::time::Duration::from_secs(120));
}

#[test]
fn job_macro_applies_defaults() {
    let job = DefaultsJob::job(ferris_core::Value::Null);
    // Name falls back to the struct name; the rest to the documented defaults.
    assert_eq!(job.name, "DefaultsJob");
    assert_eq!(job.queue, "default");
    assert_eq!(job.priority, ferris_jobs::JobPriority::Normal);
    assert_eq!(job.max_retries, 3);
    assert_eq!(job.timeout, std::time::Duration::from_secs(300));
}
