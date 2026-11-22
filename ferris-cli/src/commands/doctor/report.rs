//! Print the collected checks as a report.

use std::path::Path;

use super::health::HealthCheck;

pub(super) fn print_results(checks: &[HealthCheck], cwd: &Path) {
    println!("\n=== Ferris Health Check ===");
    println!("Path: {}", cwd.display());
    println!();

    for check in checks {
        let status = if check.passed { "✓ PASS" } else { "✗ FAIL" };
        let critical = if check.critical { " [CRITICAL]" } else { "" };
        println!("{status} {}{}", check.name, critical);
        println!("    {}", check.message);
    }

    let passed = checks.iter().filter(|c| c.passed).count();
    let total = checks.len();
    let critical_failed = checks.iter().filter(|c| !c.passed && c.critical).count();

    println!();
    println!("=== Summary ===");
    println!("Passed: {passed}/{total}");
    println!("Critical failures: {critical_failed}");
    println!();
    if critical_failed > 0 {
        println!("⚠ Critical issues found. Fix them before proceeding.");
    } else {
        println!("✓ All critical checks passed!");
    }
}
