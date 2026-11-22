//! Live stylesheet for the dev server.
//!
//! The Desk shell links `/static/desk.css`, and the server answers that route
//! from the file named by `FERRIS_DESK_CSS_DEV` when it exists, falling back to
//! the stylesheet baked into the binary. Nothing ever wrote that file, so the
//! hook was dead: editing a `.css` changed nothing until the whole web crate was
//! recompiled, which for an app means a full `cargo build` of the framework.
//!
//! The Tailwind the project already depends on (`@tailwindcss/cli`, installed
//! into `ferris-web/node_modules`) does the writing here, on every change under
//! `ferris-web/style/`. cargo-leptos cannot do it instead: it runs its own
//! downloaded Tailwind v3, which cannot parse this stylesheet.

use anyhow::{Result, bail};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Env var naming the stylesheet the server must serve in dev. Read by
/// `ferris-server`'s router; the same name the front-end supervisor exports.
pub const DESK_CSS_DEV_ENV: &str = "FERRIS_DESK_CSS_DEV";

/// Quiet time before a save burst is treated as one stylesheet build.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Compile the stylesheet now and keep it compiled until the watcher drops.
///
/// Returns the path to write into, or `None` when Tailwind is not installed: the
/// server then serves the baked stylesheet, exactly as it did before.
pub(super) fn spawn(web_dir: &Path) -> Option<PathBuf> {
    let cli = tailwind_cli(web_dir)?;
    let output = dev_css_path(web_dir);

    if let Err(e) = build(&cli, web_dir, &output) {
        warn!("{e:#}");
        return None;
    }

    // SAFETY: single-threaded startup path, before the server reads the env.
    unsafe {
        std::env::set_var(DESK_CSS_DEV_ENV, &output);
    }
    info!(css = %output.display(), "serving the Desk stylesheet from disk");

    let (tx, rx) = mpsc::channel::<Event>();
    let mut watcher = match new_watcher(tx) {
        Ok(watcher) => watcher,
        Err(e) => {
            warn!("stylesheet watcher could not start: {e:#}");
            return None;
        }
    };
    if let Err(e) = watcher.watch(&web_dir.join("style"), RecursiveMode::Recursive) {
        warn!("{}/{e}", web_dir.join("style").display());
        return None;
    }

    let web_dir = web_dir.to_path_buf();
    let built = output.clone();
    std::thread::spawn(move || {
        // The watcher lives as long as this thread: dropping it stops the
        // notifications.
        let _watcher = watcher;
        loop {
            if !settled(&rx) {
                break;
            }
            if let Err(e) = build(&cli, &web_dir, &built) {
                warn!("{e:#}");
            } else {
                info!("stylesheet rebuilt");
            }
        }
    });

    Some(output)
}

/// Where the compiled stylesheet goes: inside the framework's target dir, so it
/// is never confused with the checked-in artifact and `cargo clean` owns it.
fn dev_css_path(web_dir: &Path) -> PathBuf {
    web_dir.join("target").join("desk-dev.css")
}

/// The Tailwind CLI the project already installs, if it is installed.
fn tailwind_cli(web_dir: &Path) -> Option<PathBuf> {
    let bin_dir = web_dir.join("node_modules").join(".bin");
    let name = if cfg!(windows) {
        "tailwindcss.cmd"
    } else {
        "tailwindcss"
    };
    let cli = bin_dir.join(name);
    if cli.is_file() {
        return Some(cli);
    }
    warn!(
        "Tailwind is not installed in {}; the Desk will serve its baked stylesheet. \
         Run `npm install` in {} to get live CSS.",
        bin_dir.display(),
        web_dir.display()
    );
    None
}

/// Run Tailwind once over the framework's entry stylesheet.
fn build(cli: &Path, web_dir: &Path, output: &Path) -> Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let entry = web_dir.join("style").join("tailwind.css");
    if !entry.is_file() {
        bail!("{} does not exist", entry.display());
    }

    let status = Command::new(cli)
        .current_dir(web_dir)
        .args(["-i", "style/tailwind.css"])
        .arg("-o")
        .arg(output)
        .arg("--minify")
        .output()
        .map_err(|e| anyhow::anyhow!("could not run {}: {e}", cli.display()))?;

    if !status.status.success() {
        bail!(
            "tailwindcss failed ({}): {}",
            status.status,
            String::from_utf8_lossy(&status.stderr).trim()
        );
    }
    Ok(())
}

/// Wait for one stylesheet change, then for the rest of the burst to go quiet.
/// Returns false once the watcher closes.
fn settled(rx: &mpsc::Receiver<Event>) -> bool {
    loop {
        let Ok(event) = rx.recv() else {
            return false;
        };
        if !is_stylesheet_event(&event) {
            continue;
        }

        // Editors save in several steps (write, truncate, rename). The quiet
        // period is what turns that into a single Tailwind run.
        let mut quiet_since = Instant::now();
        while let Some(remaining) = DEBOUNCE.checked_sub(quiet_since.elapsed()) {
            match rx.recv_timeout(remaining) {
                Ok(event) if is_stylesheet_event(&event) => quiet_since = Instant::now(),
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                // The watcher went away mid-burst: build what is pending anyway,
                // the next call is the one that ends the loop.
                Err(mpsc::RecvTimeoutError::Disconnected) => return true,
            }
        }
        return true;
    }
}

fn is_stylesheet_event(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) && event
        .paths
        .iter()
        .any(|path| path.extension().is_some_and(|ext| ext == "css"))
}

fn new_watcher(tx: mpsc::Sender<Event>) -> Result<RecommendedWatcher> {
    Ok(RecommendedWatcher::new(
        move |res: std::result::Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        Config::default(),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn css_event(path: &str) -> Event {
        Event::new(EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Any,
        )))
        .add_path(PathBuf::from(path))
    }

    #[test]
    fn only_stylesheet_changes_count() {
        assert!(is_stylesheet_event(&css_event("style/tailwind.css")));
        assert!(!is_stylesheet_event(&css_event("src/lib.rs")));
        assert!(!is_stylesheet_event(&Event::new(EventKind::Other)));
    }

    #[test]
    fn a_burst_of_saves_triggers_one_build() {
        let (tx, rx) = mpsc::channel();
        let builds = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let worker = {
            let builds = builds.clone();
            std::thread::spawn(move || {
                while settled(&rx) {
                    builds.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            })
        };

        tx.send(css_event("style/tailwind.css")).unwrap();
        tx.send(css_event("style/desk.css")).unwrap();
        tx.send(css_event("style/theme.css")).unwrap();
        tx.send(css_event("src/lib.rs")).unwrap();
        drop(tx);

        worker.join().unwrap();
        assert_eq!(
            builds.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the whole burst is one stylesheet build"
        );
    }

    #[test]
    fn the_output_lands_in_the_target_dir() {
        assert_eq!(
            dev_css_path(Path::new("/srv/ferris-web")),
            PathBuf::from("/srv/ferris-web/target/desk-dev.css"),
            "the checked-in artifact must not be overwritten"
        );
    }
}
