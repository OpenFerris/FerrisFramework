//! Mirror the compiled hydrate bundle into the app project's `./pkg`.

use anyhow::Result;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// The two files that make up the hydrate bundle, in publication order: the JS
/// glue first, then the wasm it drives.
pub(super) const HYDRATE_BUNDLE_FILES: [&str; 2] = ["ferris-web.js", "ferris-web_bg.wasm"];

/// Environment override for the directory the hydrate bundle is read from.
/// Useful for CI and packaged deployments, where the framework checkout (and
/// `ferris-web/target/site`) is not next to the running binary.
const HYDRATE_SOURCE_ENV: &str = "FERRIS_HYDRATE_DIR";

/// Whether `dest` already holds exactly `src`.
///
/// The checks stay cheap on purpose: the wasm is tens of MiB, so size + mtime
/// instead of hashing the contents. An equal mtime counts as current — cargo
/// and `cargo-leptos` only rewrite the file when the build changed it.
fn is_installed(dest: &Path, src: &Path) -> bool {
    let Ok(dest_meta) = std::fs::metadata(dest) else {
        return false;
    };
    let Ok(src_meta) = std::fs::metadata(src) else {
        return false;
    };
    dest_meta.len() == src_meta.len()
        && dest_meta
            .modified()
            .ok()
            .zip(src_meta.modified().ok())
            .is_some_and(|(d, s)| d >= s)
}

/// Install one bundle file through a temporary file and a rename, so a
/// concurrent reader never observes a half-written file.
fn install(src: &Path, dest: &Path) -> Result<()> {
    let tmp = dest.with_extension("tmp");
    std::fs::copy(src, &tmp)?;
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Publish the bundle as a unit whenever any of its files is stale.
///
/// Deciding one file at a time is what let `./pkg` end up holding files from two
/// builds: a `ferris-web.js` from an older build next to a newer
/// `ferris-web_bg.wasm`, which is exactly the mismatch that makes the browser
/// fail with a `LinkError` (see `pkg_no_store` in `ferris-server`). Either every
/// file is current or the whole bundle is rewritten — glue first, because a glue
/// that provides more imports than the wasm asks for is harmless while the
/// reverse is not.
pub(super) fn publish_hydrate_bundle(target: &Path, sources: &[(&str, PathBuf)]) -> Result<usize> {
    if sources
        .iter()
        .all(|(name, src)| is_installed(&target.join(name), src))
    {
        return Ok(0);
    }
    for (name, src) in sources {
        install(src, &target.join(name))?;
    }
    Ok(sources.len())
}

/// Directories a hydrate bundle may be read from, most specific first.
///
/// The bundle is a build artifact (`wasm-bindgen` or `cargo-leptos` output) and
/// is git-ignored, so the CLI cannot embed it: doing so made `ferris-cli`
/// impossible to compile from a clean checkout. It is resolved on disk instead,
/// and an install that has none simply serves the Desk without hydration.
fn hydrate_source_candidates(project_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os(HYDRATE_SOURCE_ENV) {
        candidates.push(PathBuf::from(dir));
    }
    if let Ok(root) = crate::framework::framework_root() {
        // `wasm-bindgen --out-dir ferris-web/pkg`.
        candidates.push(root.join("ferris-web").join("pkg"));
        // `cargo leptos build` (`[package.metadata.leptos] site-root`).
        candidates.push(root.join("ferris-web").join("target/site/pkg"));
    }
    // Running straight out of the workspace: `framework_root()` finds the WIT,
    // which also covers this layout, but keeping it explicit means a relocated
    // `FERRIS_HOME` cannot silently disable hydration.
    if let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR")).parent() {
        candidates.push(root.join("ferris-web").join("pkg"));
        candidates.push(root.join("ferris-web").join("target/site/pkg"));
    }
    // Already installed by a previous `ferris dev`.
    candidates.push(project_dir.join("pkg"));
    candidates
}

/// The first directory that holds **both** bundle files.
///
/// Both are required: a directory with only the glue would be published as a
/// new bundle next to an unrelated wasm, the `LinkError` mismatch the atomic
/// publish exists to prevent.
pub(super) fn resolve_hydrate_source(
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Option<PathBuf> {
    candidates.into_iter().find(|dir| {
        HYDRATE_BUNDLE_FILES
            .iter()
            .all(|name| dir.join(name).is_file())
    })
}

/// Mirror the compiled hydrate bundle into the app project's `./pkg` so the web
/// role can serve it at `/pkg`. Rebuilding `ferris-web` refreshes the whole pair
/// instead of leaving a stale copy behind for the rest of the session.
///
/// No bundle anywhere is not an error: the Desk renders server-side and the
/// browser never hydrates, which is a working (if less interactive) Desk. The
/// warning names the exact command and the override, because a silent
/// SSR-only dev server looks like a broken front end.
pub(super) fn ensure_hydrate_bundle(project_dir: &Path) -> Result<()> {
    ensure_hydrate_bundle_from(project_dir, hydrate_source_candidates(project_dir))
}

/// The half of [`ensure_hydrate_bundle`] that does not read the environment.
///
/// Split out so the "nothing to install" case is testable without the framework
/// checkout the test binary happens to sit next to.
pub(super) fn ensure_hydrate_bundle_from(
    project_dir: &Path,
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<()> {
    let Some(source) = resolve_hydrate_source(candidates) else {
        warn!(
            "hydrate bundle not found (looked in {}, ferris-web/pkg, ferris-web/target/site/pkg); \
             the Desk will render server-side only. Build it optimized — a debug wasm is ~145 MiB \
             over the wire — with: cargo build -p ferris-web --lib --release --no-default-features \
             --features hydrate --target wasm32-unknown-unknown && wasm-bindgen --target web \
             --out-dir ferris-web/pkg --out-name ferris-web \
             target/wasm32-unknown-unknown/release/ferris_web.wasm  (or set {HYDRATE_SOURCE_ENV})",
            project_dir.join("pkg").display()
        );
        return Ok(());
    };

    let target = project_dir.join("pkg");
    std::fs::create_dir_all(&target)?;

    let sources: Vec<(&str, PathBuf)> = HYDRATE_BUNDLE_FILES
        .iter()
        .map(|name| (*name, source.join(name)))
        .collect();
    let refreshed = publish_hydrate_bundle(&target, &sources)?;
    if refreshed > 0 {
        info!(
            files = refreshed,
            "hydrate bundle refreshed in {}",
            target.display()
        );
    }
    Ok(())
}
