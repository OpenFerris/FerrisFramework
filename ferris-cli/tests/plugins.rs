//! End-to-end proof for H2: the apps `ferris serve`/`run` publish at boot.
//!
//! `ferris-cli` exposes its command modules as a library, so these drive the
//! very function `serve::start` calls before `Server::run` — not a copy of it.
//! The component fixture is `ferris-wasm`'s `probe-ok`; when it was never built
//! (fresh clone) the tests skip **loudly** instead of failing, the same pattern
//! `ferris-wasm/tests/typed_request.rs` uses.

use ferris_cli::commands::plugin_boot::{self, AppComponent};
use ferris_core::AppsConfig;
use std::path::{Path, PathBuf};

/// A directory of this test's own, so parallel tests cannot collide.
fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ferris-plugins-it-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `probe-ok` as the WASM tests build it.
fn probe_ok() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ferris-wasm/tests/fixtures/probe-ok/target/wasm32-wasip2/debug/probe_ok.wasm")
}

/// The library boot path needs a runtime it can `block_on`; the guest's
/// `describe()` parks a worker with `block_in_place`, which a current-thread
/// runtime rejects, so this must be multi-threaded like `serve::start`'s.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("multi-thread runtime")
}

/// A `plugins/<name>.wasm` directory holding two copies of the fixture, so the
/// multi-app path is exercised rather than the single-plugin one.
fn fixtures(root: &Path) -> bool {
    let fixture = probe_ok();
    if !fixture.is_file() {
        eprintln!(
            "SKIPPED: cargo build --target wasm32-wasip2 --manifest-path \
             ferris-wasm/tests/fixtures/probe-ok/Cargo.toml"
        );
        return false;
    }
    let apps_dir = root.join("plugins");
    std::fs::create_dir_all(&apps_dir).unwrap();
    std::fs::copy(&fixture, apps_dir.join("probe-ok.wasm")).unwrap();
    std::fs::copy(&fixture, apps_dir.join("probe-other.wasm")).unwrap();
    true
}

/// The regression this pins: `ferris serve` used to answer with no `/plugins`
/// route and no `/api/method` handler because `ServerSettings.plugins` was
/// always `None`. `bootstrap` is what now fills it before the router exists.
#[test]
fn bootstrap_publishes_every_component_in_the_directory() {
    let root = scratch_dir("bootstrap");
    if !fixtures(&root) {
        return;
    }

    let rt = runtime();
    let pool = rt.block_on(ferris_db::memory_db()).unwrap();
    let store = plugin_boot::bootstrap(&rt, pool, &AppsConfig::default(), &root)
        .expect("a readable directory of components must boot")
        .expect("two components were found");

    let mut names: Vec<String> = store
        .plugins()
        .iter()
        .map(|plugin| plugin.name().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["probe-ok", "probe-other"],
        "every component in the directory is published, one app per file"
    );
    assert_eq!(
        store.name().as_deref(),
        Some("probe-other"),
        "discovery is alphabetical, so the last published app is deterministic"
    );

    std::fs::remove_dir_all(&root).unwrap();
}

/// An operator who lists an app in `[apps] enabled` must not get a server that
/// silently serves without it.
#[test]
fn a_missing_enabled_app_fails_before_the_server_starts() {
    let root = scratch_dir("missing-enabled");
    let apps_dir = root.join("plugins");
    std::fs::create_dir_all(&apps_dir).unwrap();

    let apps = AppsConfig {
        enabled: vec!["ghost".to_string()],
        ..AppsConfig::default()
    };
    let rt = runtime();
    let pool = rt.block_on(ferris_db::memory_db()).unwrap();

    let err = plugin_boot::bootstrap(&rt, pool, &apps, &root)
        .expect_err("an enabled app with no component must abort the boot");
    let message = err.to_string();
    assert!(message.contains("ghost"), "{message}");
    assert!(
        message.contains(&apps_dir.display().to_string()),
        "{message}"
    );

    std::fs::remove_dir_all(&root).unwrap();
}

/// A stock install has no `plugins/` directory and is perfectly valid: the
/// framework Desk alone. Only an explicit `[apps] enabled` makes it fatal.
#[test]
fn an_install_without_a_plugin_directory_boots_without_apps() {
    let root = scratch_dir("no-directory");

    let rt = runtime();
    let pool = rt.block_on(ferris_db::memory_db()).unwrap();
    assert!(
        plugin_boot::bootstrap(&rt, pool, &AppsConfig::default(), &root)
            .unwrap()
            .is_none(),
        "no directory, no apps, no error"
    );

    let apps = AppsConfig {
        enabled: vec!["ghost".to_string()],
        ..AppsConfig::default()
    };
    let rt = runtime();
    let pool = rt.block_on(ferris_db::memory_db()).unwrap();
    let err = plugin_boot::bootstrap(&rt, pool, &apps, &root)
        .expect_err("an explicit app list with no directory is an operator error");
    assert!(err.to_string().contains("not a directory"), "{err}");

    std::fs::remove_dir_all(&root).unwrap();
}

/// Move the E2E one register higher and assert the default `[apps]` contract
/// `ferris new` writes: the server reads exactly where `ferris build` puts the
/// component, and a package with a dash keeps it as the plugin name.
#[test]
fn the_build_output_directory_is_what_serve_reads() {
    assert_eq!(
        plugin_boot::plugins_dir(Path::new("/srv/app"), &AppsConfig::default()),
        PathBuf::from("/srv/app/plugins")
    );

    let root = scratch_dir("dash");
    let apps_dir = root.join("plugins");
    std::fs::create_dir_all(&apps_dir).unwrap();
    std::fs::write(apps_dir.join("my-app.wasm"), b"\0asm").unwrap();

    let found = plugin_boot::discover(&apps_dir, &[]).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0],
        AppComponent {
            name: "my-app".to_string(),
            path: apps_dir.join("my-app.wasm"),
        }
    );

    std::fs::remove_dir_all(&root).unwrap();
}
