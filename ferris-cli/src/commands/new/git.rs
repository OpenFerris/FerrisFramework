//! `git init` + the initial commit, best effort.

use std::path::Path;
use tracing::warn;

pub(super) fn init_git(project_dir: &Path) -> anyhow::Result<()> {
    if !git(project_dir, &["init", "--initial-branch=main"]) {
        warn!("git init failed; skipping repository initialization");
        return Ok(());
    }
    if !git(project_dir, &["add", "."]) {
        warn!("git add . failed; skipping initial commit");
        return Ok(());
    }
    if !git(project_dir, &["commit", "-m", "Initial commit"]) {
        warn!(
            "git commit failed; this usually means git identity is not configured
(set user.name / user.email) — the project is still usable"
        );
    }
    Ok(())
}

/// Runs a git subcommand and returns whether it succeeded. Logs stderr on
/// failure so the user sees the real cause instead of a generic message.
fn git(dir: &Path, args: &[&str]) -> bool {
    match std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
    {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            warn!("git {:?} failed: {}", args, stderr.trim());
            false
        }
        Err(e) => {
            warn!("failed to spawn git: {e}");
            false
        }
    }
}
