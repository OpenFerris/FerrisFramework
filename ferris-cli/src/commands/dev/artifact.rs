//! Whether the last component build contains the newest sources.

use std::path::Path;
use std::time::SystemTime;

use super::project::app_core_wasm_path;

/// Whether the last artifact is at least as new as every source that feeds it.
///
/// The watch loop uses this after dropping the events that piled up during a
/// rebuild: if a source is newer than the artifact, cargo read it too late and
/// that edit is not in the component the dev server just published.
pub(crate) fn artifact_is_current(project_dir: &Path, package_name: &str) -> bool {
    let artifact = app_core_wasm_path(project_dir, package_name, "dev");
    let Ok(built_at) = std::fs::metadata(&artifact).and_then(|m| m.modified()) else {
        // No artifact to compare against: let the rebuild fail with a real error.
        return true;
    };
    artifact_covers_sources(built_at, newest_source_mtime(project_dir))
}

/// Split out from [`artifact_is_current`] so the decision is testable without
/// racing the filesystem clock.
pub(super) fn artifact_covers_sources(
    built_at: SystemTime,
    newest_source: Option<SystemTime>,
) -> bool {
    // Strictly older only: on filesystems with coarse timestamps an equal mtime
    // is not evidence of an edit cargo could have missed.
    newest_source.is_none_or(|newest| newest <= built_at)
}

/// Newest mtime across the roots the component is built from.
pub(super) fn newest_source_mtime(project_dir: &Path) -> Option<SystemTime> {
    let mut newest = walk_newest(&project_dir.join("src"), None);
    newest = walk_newest(&project_dir.join("wit"), newest);
    for manifest in ["Cargo.toml", "Cargo.lock"] {
        let modified = std::fs::metadata(project_dir.join(manifest))
            .and_then(|m| m.modified())
            .ok();
        newest = newer(newest, modified);
    }
    newest
}

fn newer(a: Option<SystemTime>, b: Option<SystemTime>) -> Option<SystemTime> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (None, b) => b,
        (a, None) => a,
    }
}

/// Newest mtime under `dir`, ignoring anything unreadable.
fn walk_newest(dir: &Path, seen: Option<SystemTime>) -> Option<SystemTime> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return seen;
    };
    let mut newest = seen;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        newest = newer(newest, meta.modified().ok());
        if meta.is_dir() {
            newest = walk_newest(&entry.path(), newest);
        }
    }
    newest
}
