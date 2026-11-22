//! The feature-matrix check: what this binary compiled vs what the config asks
//! for.
//!
//! `doctor` is where an operator looks after a `serve` refused to boot, so this
//! check always prints both sides — even when they agree — instead of only
//! speaking up on failure. A mismatch is **critical**: it is the same condition
//! `ferris serve`/`run` reject, and a report that said "all critical checks
//! passed" while boot was impossible would be worse than no report.

use crate::config::CliConfig;
use crate::features;

use super::health::HealthCheck;

const NAME: &str = "Feature matrix";

pub(super) fn check_features(config: &CliConfig) -> HealthCheck {
    let compiled = features::compiled();
    let mismatches = features::mismatches(config, &compiled);

    let compiled_label = features::describe(&compiled);
    let configured = features::configured_summary(&config.features);

    if mismatches.is_empty() {
        let message = format!("compiled: {compiled_label}; configured: {configured}");
        return HealthCheck {
            name: NAME.to_string(),
            passed: true,
            message,
            critical: true,
        };
    }

    let reasons = mismatches
        .iter()
        .map(features::FeatureMismatch::reason)
        .collect::<Vec<_>>()
        .join("; ");
    HealthCheck {
        name: NAME.to_string(),
        passed: false,
        message: format!("compiled: {compiled_label}; unknown to this binary: {reasons}"),
        critical: true,
    }
}
