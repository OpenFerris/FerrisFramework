//! File watcher, debounce and the reload loop.

use anyhow::Result;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::ffi::OsStr;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
use tracing::info;

pub(crate) fn start_watcher(dir: &Path, tx: mpsc::Sender<Event>) -> Result<RecommendedWatcher> {
    // Watches the app sources so `--hot-reload app` (in-process) and
    // `--hot-reload both` (frontend dev server) can rebuild + swap the plugin.
    // No poll interval here: `RecommendedWatcher` is inotify/kqueue on these
    // platforms and reads no such config, only `PollWatcher` does.
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        Config::default(),
    )?;

    watcher.watch(&dir.join("src"), RecursiveMode::Recursive)?;
    watcher.watch(&dir.join("wit"), RecursiveMode::Recursive)?;
    watcher.watch(&dir.join("Cargo.toml"), RecursiveMode::NonRecursive)?;
    // Optional: an app without a committed lockfile has nothing to watch here.
    let _ = watcher.watch(&dir.join("Cargo.lock"), RecursiveMode::NonRecursive);

    info!("Watching for changes in src/, wit/, Cargo.toml, Cargo.lock");
    Ok(watcher)
}

pub(super) fn should_rebuild(event: &Event) -> bool {
    if !matches!(
        event.kind,
        notify::EventKind::Create(_) | notify::EventKind::Modify(_) | notify::EventKind::Remove(_)
    ) {
        return false;
    }
    // The watched roots are `src/`, `wit/`, `Cargo.toml` and `Cargo.lock`, but
    // editors and tools drop plenty of files in there that cannot change the
    // component (swap files, `~` backups, `.gitignore`, fixtures). A rebuild
    // costs a full `cargo build`, so only real sources get through.
    event.paths.iter().any(|path| is_source_file(path))
}

/// Whether a path is a file whose contents actually reach the component.
fn is_source_file(path: &Path) -> bool {
    if !matches!(
        path.extension().and_then(OsStr::to_str),
        Some("rs" | "wit" | "toml" | "lock")
    ) {
        return false;
    }
    // `.#lib.rs` (vim), `#lib.rs#` (emacs), `lib.rs~` and `~lib.rs` (backup copies)
    // are scratch files that still carry a `.rs` extension.
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    !(name.starts_with('.')
        || name.starts_with('#')
        || name.starts_with('~')
        || name.ends_with('~'))
}

/// Window used to collapse the burst of events a single save produces.
pub(super) const DEBOUNCE: Duration = Duration::from_millis(250);

/// Block until a rebuild-worthy change arrives.
///
/// Saving a file emits several raw events (write, rename, metadata), so this
/// returns once the burst has been quiet for [`DEBOUNCE`]; edits made while a
/// build is running stay queued for the caller to drop or consume.
pub(super) fn recv_change(rx: &mpsc::Receiver<Event>) -> Option<Event> {
    let mut last = loop {
        match rx.recv() {
            Ok(event) if should_rebuild(&event) => break event,
            // Anything that cannot change the component keeps the loop waiting.
            Ok(_) => {}
            Err(_) => return None,
        }
    };
    while let Ok(event) = rx.recv_timeout(DEBOUNCE) {
        if should_rebuild(&event) {
            last = event;
        }
    }
    Some(last)
}

/// Drop every queued event, returning how many there were.
///
/// The caller is about to (or has just) run a build that already contains all of
/// them — except for edits that landed after cargo read the file, which is why
/// the caller checks the artifact's mtime before believing the queue is empty.
pub(super) fn drain_pending(rx: &mpsc::Receiver<Event>) -> usize {
    let mut dropped = 0;
    while rx.try_recv().is_ok() {
        dropped += 1;
    }
    dropped
}

/// Watch loop shared by `--hot-reload app` and the app watcher of `--hot-reload
/// both`. Rebuilds once per settled change, and returns when the watcher closes.
///
/// `artifact_is_current` is asked, but only when events piled up *during* a
/// rebuild, whether the artifact really contains them: an edit saved while
/// cargo was already reading that file is not compiled in, and dropping its
/// event would leave the change out of the dev server until the next save.
pub(crate) fn watch_and_reload(
    rx: &mpsc::Receiver<Event>,
    mut on_change: impl FnMut(&Event),
    mut rebuild: impl FnMut(),
    artifact_is_current: impl Fn() -> bool,
) {
    while let Some(event) = recv_change(rx) {
        on_change(&event);
        rebuild();
        // Draining after `rebuild` rather than before `recv_change` is what
        // makes a save burst collapse into one build: waiting first would have
        // already taken the head of the queue and scheduled a second, redundant
        // full build for an edit that is compiled in.
        let dropped = drain_pending(rx);
        // One catch-up build covers the edits this rebuild raced. Bounded to a
        // single extra build per burst: cargo not rewriting an unchanged artifact
        // would otherwise ask for the same build forever.
        if dropped > 0 && !artifact_is_current() {
            info!(
                dropped,
                "sources are newer than the artifact; rebuilding once more"
            );
            on_change(&event);
            rebuild();
            drain_pending(rx);
        }
    }
}
