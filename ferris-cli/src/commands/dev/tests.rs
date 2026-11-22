use super::artifact::{artifact_covers_sources, newest_source_mtime};
use super::hydrate::{
    HYDRATE_BUNDLE_FILES, ensure_hydrate_bundle_from, publish_hydrate_bundle,
    resolve_hydrate_source,
};
use super::project::app_core_wasm_path;
use super::watch::{DEBOUNCE, drain_pending, recv_change, should_rebuild};
use super::*;
use notify::Event;
use notify::EventKind;
use notify::event::{ModifyKind, RemoveKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn write(path: &str) -> Event {
    Event::new(EventKind::Modify(ModifyKind::Any)).add_path(PathBuf::from(path))
}

/// Upper bound for anything the watch loop has to reach, so a regression
/// fails the test instead of hanging the suite.
const TIMEOUT: Duration = Duration::from_secs(10);

#[test]
fn rebuilds_on_source_edits_in_watched_roots() {
    for path in [
        "src/lib.rs",
        "src/desk/page.rs",
        "wit/ferris.wit",
        "Cargo.toml",
        "Cargo.lock",
    ] {
        assert!(
            should_rebuild(&write(path)),
            "{path} should trigger a rebuild"
        );
    }
}

#[test]
fn ignores_files_that_cannot_change_the_component() {
    // Editor scratch files, docs and fixtures live under the same roots.
    for path in [
        "src/.#lib.rs",
        "src/#lib.rs#",
        "src/lib.rs~",
        "src/lib.rs.swp",
        "src/~lib.rs",
        "README.md",
        "src/style.css",
    ] {
        assert!(
            !should_rebuild(&write(path)),
            "{path} should not trigger a rebuild"
        );
    }
    assert!(!should_rebuild(&Event::new(EventKind::Access(
        notify::event::AccessKind::Read
    ))));
    assert!(!should_rebuild(&Event::new(EventKind::Other)));
}

#[test]
fn accepts_creating_and_removing_sources() {
    assert!(should_rebuild(
        &Event::new(EventKind::Create(notify::event::CreateKind::File))
            .add_path(PathBuf::from("src/new.rs"))
    ));
    assert!(should_rebuild(
        &Event::new(EventKind::Remove(RemoveKind::File)).add_path(PathBuf::from("src/gone.rs"))
    ));
}

#[test]
fn a_save_burst_collapses_into_one_rebuild() {
    let (tx, rx) = mpsc::channel();
    // One editor save: write, then a metadata touch.
    tx.send(write("src/lib.rs")).unwrap();
    tx.send(write("src/lib.rs")).unwrap();

    let event = recv_change(&rx).expect("a change is pending");
    assert!(event.paths.iter().any(|p| p.ends_with("src/lib.rs")));

    // Nothing extra is left over, so the loop cannot rebuild twice.
    drain_pending(&rx);
    assert!(
        rx.try_recv().is_err(),
        "burst must not schedule a second build"
    );
}

#[test]
fn edits_landed_during_a_build_do_not_schedule_a_second_rebuild() {
    let (tx, rx) = mpsc::channel();
    // The fake rebuild parks until the test releases it, so the events below
    // provably land while a build is still in flight.
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let rebuilds = Arc::new(AtomicUsize::new(0));
    let edits = Arc::new(AtomicUsize::new(0));

    let worker = {
        let (rebuilds, edits) = (rebuilds.clone(), edits.clone());
        thread::spawn(move || {
            watch_and_reload(
                &rx,
                |_| {
                    edits.fetch_add(1, Ordering::SeqCst);
                },
                || {
                    // Only the first build parks; later ones run straight
                    // through so the loop can reach the channel close.
                    let build = rebuilds.fetch_add(1, Ordering::SeqCst);
                    if build == 0 {
                        started_tx.send(()).unwrap();
                        done_rx.recv_timeout(TIMEOUT).unwrap();
                    }
                },
                // The artifact is up to date, so those edits are in it.
                || true,
            )
        })
    };

    tx.send(write("src/lib.rs")).unwrap();
    started_rx
        .recv_timeout(TIMEOUT)
        .expect("the first save starts a build");

    // Three saves while that build is still running: the artifact check says
    // the build already contains them, so no second build is scheduled.
    tx.send(write("src/desk/page.rs")).unwrap();
    tx.send(write("src/lib.rs")).unwrap();
    tx.send(write("src/lib.rs")).unwrap();
    done_tx.send(()).unwrap();
    drop(tx); // close the channel so the loop can return

    worker.join().unwrap();
    assert_eq!(
        rebuilds.load(Ordering::SeqCst),
        1,
        "edits that landed during a rebuild must not trigger another build"
    );
    assert_eq!(edits.load(Ordering::SeqCst), 1);
}

#[test]
fn an_edit_the_build_missed_is_rebuilt_once() {
    let (tx, rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let rebuilds = Arc::new(AtomicUsize::new(0));

    let worker = {
        let rebuilds = rebuilds.clone();
        thread::spawn(move || {
            watch_and_reload(
                &rx,
                |_| {},
                || {
                    let build = rebuilds.fetch_add(1, Ordering::SeqCst);
                    if build == 0 {
                        started_tx.send(()).unwrap();
                        done_rx.recv_timeout(TIMEOUT).unwrap();
                    }
                },
                // This rebuild read the files before the saves below landed, and
                // so does the catch-up build: cargo never rewrites the artifact.
                // The loop must still stop at one extra build.
                || false,
            )
        })
    };

    tx.send(write("src/lib.rs")).unwrap();
    started_rx
        .recv_timeout(TIMEOUT)
        .expect("the first save starts a build");
    tx.send(write("src/desk/page.rs")).unwrap();
    tx.send(write("src/lib.rs")).unwrap();
    done_tx.send(()).unwrap();

    wait_for(
        || rebuilds.load(Ordering::SeqCst) == 2,
        "the catch-up build for the edit the first one missed",
    );
    // Anything more than one extra build here is the loop this rule exists
    // to prevent.
    thread::sleep(DEBOUNCE * 2);
    assert_eq!(
        rebuilds.load(Ordering::SeqCst),
        2,
        "an edit newer than the artifact must be rebuilt, once"
    );

    drop(tx);
    worker.join().unwrap();
}

#[test]
fn a_change_after_a_rebuild_settles_gets_its_own_rebuild() {
    let (tx, rx) = mpsc::channel();
    let rebuilds = Arc::new(AtomicUsize::new(0));
    let worker = {
        let rebuilds = rebuilds.clone();
        thread::spawn(move || {
            watch_and_reload(
                &rx,
                |_| {},
                || {
                    rebuilds.fetch_add(1, Ordering::SeqCst);
                },
                || true,
            );
        })
    };

    tx.send(write("src/lib.rs")).unwrap();
    wait_for(|| rebuilds.load(Ordering::SeqCst) == 1, "the first rebuild");
    // The loop drains, then parks in `recv_change` for `DEBOUNCE`. Once
    // that has elapsed the next edit is genuinely new, not an in-flight one.
    thread::sleep(DEBOUNCE * 3);
    tx.send(write("src/lib.rs")).unwrap();
    wait_for(
        || rebuilds.load(Ordering::SeqCst) == 2,
        "the second rebuild",
    );

    drop(tx);
    worker.join().unwrap();
    assert_eq!(rebuilds.load(Ordering::SeqCst), 2);
}

/// Fail instead of hanging when the watch loop never gets where we expect.
fn wait_for(cond: impl Fn() -> bool, what: &str) {
    let deadline = Instant::now() + TIMEOUT;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_burst_that_settles_returns_promptly() {
    let (tx, rx) = mpsc::channel();
    let started = Instant::now();
    tx.send(write("src/lib.rs")).unwrap();
    assert!(recv_change(&rx).is_some());
    // Debouncing waits for the burst to go quiet, not for a fixed timeout.
    assert!(started.elapsed() < DEBOUNCE * 10);
}

#[test]
fn an_artifact_older_than_a_source_is_not_current() {
    let now = std::time::SystemTime::now();
    let earlier = now - Duration::from_secs(1);
    let later = now + Duration::from_secs(1);

    assert!(
        artifact_covers_sources(later, Some(now)),
        "a fresh artifact is current"
    );
    assert!(
        !artifact_covers_sources(earlier, Some(now)),
        "a source newer than the artifact is not"
    );
    // Coarse filesystem timestamps: equal mtimes are not evidence.
    assert!(artifact_covers_sources(now, Some(now)));
    assert!(artifact_covers_sources(earlier, None), "no sources at all");
}

#[test]
fn the_component_lands_where_the_watcher_looks_for_it() {
    let path = app_core_wasm_path(Path::new("/srv/app"), "my-app", "dev");
    assert!(
        path.ends_with("target/wasm32-wasip2/debug/my_app.wasm"),
        "{path:?}"
    );
}

#[test]
fn a_built_artifact_covers_every_source_it_was_built_from() {
    let dir = scratch_dir("artifact-covers");
    std::fs::create_dir_all(dir.join("src/desk")).unwrap();
    std::fs::create_dir_all(dir.join("wit")).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "// a").unwrap();
    std::fs::write(dir.join("src/desk/page.rs"), "// b").unwrap();
    std::fs::write(dir.join("wit/extra.wit"), "package ferris:app;").unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]").unwrap();

    // Sources only, no artifact yet: nothing to compare, let the rebuild speak.
    assert!(artifact_is_current(&dir, "scratch"));
    std::fs::create_dir_all(dir.join("target/wasm32-wasip2/debug")).unwrap();
    std::fs::write(
        app_core_wasm_path(&dir, "scratch", "dev"),
        b"\0asm\x0d\x00\x01\x00",
    )
    .unwrap();

    assert!(
        artifact_is_current(&dir, "scratch"),
        "an artifact written after the sources covers them"
    );
    assert!(newest_source_mtime(&dir).is_some());
    // A package name is normalized the same way in both places.
    assert!(
        artifact_is_current(&dir, "scratch-app"),
        "unused name must not panic"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A directory of this test's own, so parallel tests cannot collide.
fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ferris-dev-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Write the two bundle files of a directory.
fn write_bundle(dir: &Path, glue: &[u8], wasm: &[u8]) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("ferris-web.js"), glue).unwrap();
    std::fs::write(dir.join("ferris-web_bg.wasm"), wasm).unwrap();
}

/// The two hydrate files as `publish_hydrate_bundle` receives them.
fn bundle_sources(root: &Path) -> Vec<(&'static str, PathBuf)> {
    HYDRATE_BUNDLE_FILES
        .iter()
        .map(|name| (*name, root.join(name)))
        .collect()
}

/// The regression this pins: a `ferris-web.js` of the *same length* but from
/// an older build used to be considered current (length-only check) while a
/// wasm of a different length was rewritten, leaving `./pkg` with a glue and
/// a wasm from two builds — the mismatch that aborts the page with
/// `LinkError: import object field ... is not a Function`. The bundle is
/// published as a unit now, so a stale glue refreshes the wasm too.
#[test]
fn a_stale_glue_refreshes_the_whole_bundle() {
    let root = scratch_dir("bundle-pair");
    let pkg = root.join("pkg");
    let src = root.join("src");
    std::fs::create_dir_all(&pkg).unwrap();

    // Same length as the source glue, different bytes: an older build.
    std::fs::write(pkg.join("ferris-web.js"), b"old-glue").unwrap();
    // Different length, so a size-only check would rewrite exactly this.
    std::fs::write(pkg.join("ferris-web_bg.wasm"), b"old").unwrap();
    write_bundle(&src, b"new-glue", b"new-wasm");

    let written = publish_hydrate_bundle(&pkg, &bundle_sources(&src)).unwrap();

    assert_eq!(written, 2, "a stale member must publish the whole bundle");
    assert_eq!(
        std::fs::read(pkg.join("ferris-web.js")).unwrap(),
        b"new-glue",
        "the stale same-length glue must be replaced"
    );
    assert_eq!(
        std::fs::read(pkg.join("ferris-web_bg.wasm")).unwrap(),
        b"new-wasm",
        "the wasm must come from the same build as the glue"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// A bundle that already matches is left alone: no rewrite, no `mtime` churn.
#[test]
fn a_current_bundle_is_not_rewritten() {
    let root = scratch_dir("bundle-current");
    let pkg = root.join("pkg");
    let src = root.join("src");
    write_bundle(&src, b"glue", b"wasm");
    std::fs::create_dir_all(&pkg).unwrap();

    assert_eq!(
        publish_hydrate_bundle(&pkg, &bundle_sources(&src)).unwrap(),
        2
    );
    assert_eq!(
        publish_hydrate_bundle(&pkg, &bundle_sources(&src)).unwrap(),
        0,
        "the second pass must not touch an up-to-date bundle"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// The bundle is compared on size and mtime: rebuilding `ferris-web`
/// (a newer source) republishes the pair, and the installed copy is
/// byte-identical to the checkout.
#[test]
fn file_sources_refresh_from_a_newer_checkout() {
    let root = scratch_dir("bundle-file");
    let src = root.join("src");
    let pkg = root.join("pkg");
    write_bundle(&src, b"glue", b"wasm");
    std::fs::create_dir_all(&pkg).unwrap();

    assert_eq!(
        publish_hydrate_bundle(&pkg, &bundle_sources(&src)).unwrap(),
        2
    );
    assert_eq!(std::fs::read(pkg.join("ferris-web.js")).unwrap(), b"glue");
    assert_eq!(
        publish_hydrate_bundle(&pkg, &bundle_sources(&src)).unwrap(),
        0,
        "an installed copy at least as new as the source is kept"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// `ferris dev` in a project that already has `./pkg` must not copy the
/// bundle onto itself: the same directory is a valid candidate and a copy
/// would defeat the freshness check.
#[test]
fn a_bundle_already_in_place_is_left_alone() {
    let root = scratch_dir("bundle-in-place");
    let pkg = root.join("pkg");
    write_bundle(&pkg, b"glue", b"wasm");

    assert_eq!(
        publish_hydrate_bundle(&pkg, &bundle_sources(&pkg)).unwrap(),
        0,
        "source == target must publish nothing"
    );
    assert_eq!(std::fs::read(pkg.join("ferris-web.js")).unwrap(), b"glue");
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_first_candidate_with_both_files_wins() {
    let root = scratch_dir("bundle-candidates");
    let only_glue = root.join("only-glue");
    let complete = root.join("complete");
    std::fs::create_dir_all(&only_glue).unwrap();
    std::fs::write(only_glue.join("ferris-web.js"), b"glue").unwrap();
    write_bundle(&complete, b"glue", b"wasm");

    let resolved = resolve_hydrate_source(vec![only_glue, complete.clone()]);
    assert_eq!(resolved, Some(complete), "a half bundle is not a bundle");
    std::fs::remove_dir_all(&root).unwrap();
}

/// No bundle anywhere is not an error: the Desk renders server-side and the
/// caller gets a warning, never a failed dev server. What must not happen is
/// a `./pkg` directory created from nothing.
#[test]
fn no_bundle_anywhere_is_an_honest_ssr_only_fallback() {
    let root = scratch_dir("bundle-absent");
    std::fs::create_dir_all(&root).unwrap();

    assert_eq!(resolve_hydrate_source(Vec::new()), None);
    ensure_hydrate_bundle_from(&root, Vec::new()).expect("SSR-only is a valid dev server");
    assert!(
        !root.join("pkg").exists(),
        "an absent bundle must not leave an empty pkg/ behind"
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// `ferris dev` must boot on a fresh `ferris new` project even though the
/// scaffold deliberately writes no credential. The fill is dev-only: it never
/// touches the caller's config (so `serve`/`run` keep failing closed) and it
/// never writes the secret to disk.
#[test]
fn dev_fills_an_ephemeral_secret_without_touching_the_input() {
    let mut config = crate::config::CliConfig::default();
    config.auth.jwt_secret = None;

    let dev = dev_config(&config);

    let secret = dev.auth.jwt_secret.as_deref().expect("a dev secret");
    assert!(secret.len() >= 32, "{secret}");
    assert!(
        config.auth.jwt_secret.is_none(),
        "the caller's config must stay unmodified"
    );
}

/// A configured secret is used verbatim; the production requirement is not
/// weakened by the development exception.
#[test]
fn dev_keeps_a_configured_secret_and_the_production_path_still_refuses() {
    let mut config = crate::config::CliConfig::default();
    let configured = "a".repeat(32);
    config.auth.jwt_secret = Some(configured.clone());

    let dev = dev_config(&config);

    assert_eq!(dev.auth.jwt_secret.as_deref(), Some(configured.as_str()));
    assert!(
        crate::config::CliConfig::default()
            .require_jwt_secret()
            .is_err(),
        "serve/run must still fail without an operator secret"
    );
}
