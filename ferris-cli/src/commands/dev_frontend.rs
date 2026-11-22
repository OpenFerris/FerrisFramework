//! `ferris dev --hot-reload front|both`: supervisor directly driven by `cargo-leptos`.
//!
//! Delegates watching, WASM compilation, Tailwind processing, live-reload WebSockets,
//! and SSR server lifecycle directly to `cargo-leptos` internal engines. The SSR
//! dev server (a `ferris-web` bin) additionally mounts the project's app plugin so
//! the Desk lists the app's modules; `--hot-reload both` rebuilds and hot-swaps
//! that plugin through a loopback reload endpoint when the app sources change.

use anyhow::Result;
use cargo_leptos::config::Cli;
use clap::Parser;
use notify::Event;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tracing::{info, warn};

use super::DevArgs;
use crate::commands::HotReloadMode;
use crate::commands::serve;
use crate::config::CliConfig;
use crate::framework::framework_root;
use ferris_core::{DatabaseType, SslMode};
use ferris_server::{ServerRole, ServerRoles};

/// Guarantee the browser is opened only once per supervisor run.
static OPEN_SENT: AtomicUsize = AtomicUsize::new(0);

pub fn run(args: DevArgs, config: &CliConfig, verbose: bool) -> Result<()> {
    // `dev::run` already filled an ephemeral secret before delegating here; a
    // direct call still gets the same dev-only treatment (and the warning), so
    // the supervisor child and the app-only path never disagree.
    let config = super::dev::dev_config(config);
    let project_dir = std::env::current_dir()?;
    super::dev::check_ferris_project(&project_dir)?;
    let package_name = super::build::package_name(&project_dir)?;

    // Locate framework and frontend directory
    if !config.cli.unlock_source {
        warn!(
            "frontend hot-reload requires `cli.unlock_source = true` (or FERRIS_UNLOCK_SOURCE=1)"
        );
        return run_app_only(args, &config, verbose);
    }

    let framework = match framework_root() {
        Ok(root) => root,
        Err(e) => {
            warn!("frontend hot-reload unavailable (no framework source found): {e}");
            return run_app_only(args, &config, verbose);
        }
    };

    let web_dir = framework.join("ferris-web");
    let web_cargo = web_dir.join("Cargo.toml");

    if !web_cargo.is_file() {
        warn!(
            "ferris-web source not found at {}; falling back to app-only hot-reload",
            web_dir.display()
        );
        return run_app_only(args, &config, verbose);
    }

    info!("Frontend hot-reload: powered by cargo-leptos engine");
    info!("Watching {}", web_dir.display());

    // Before the first build, so the app plugin and the cargo-leptos pipeline
    // agree on cargo settings and neither one pays a cold rebuild later.
    apply_cargo_quiet(verbose);

    // cargo-leptos reads its bind address from `[package.metadata.leptos]`
    // (`site-addr`), not from these flags, so `--host`/`--port` used to be
    // silently ignored here: the browser opened on one port while the dev server
    // listened on another.
    apply_site_addr(&web_cargo, &args.host, args.port)?;

    // Open the browser once if requested
    if args.open {
        let url = format!("http://{}:{}", args.host, args.port);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1200));
            if OPEN_SENT.fetch_add(1, Ordering::Relaxed) == 0 {
                info!("opening {}", url);
                let _ = open::that(&url);
            }
        });
    }

    // Resolve the project config exactly like the real server would and forward
    // it to the cargo-leptos SSR child (which runs with the framework as CWD and
    // cannot see the project's `config/ferris.toml`).
    let roles = ServerRoles::parse(&[
        ServerRole::Api.as_str().to_string(),
        ServerRole::Web.as_str().to_string(),
        ServerRole::Realtime.as_str().to_string(),
    ])?;
    let settings = serve::settings_for(args.host.clone(), args.port, roles, &config)?;
    forward_dev_env(&settings);

    // The Desk can only show the app's modules if the app WASM plugin is loaded
    // by the SSR dev server, so build it once and point the server at it.
    let core_wasm = match build_app_plugin(&project_dir, &package_name, verbose) {
        Ok(path) => Some(path),
        Err(e) => {
            warn!("app plugin could not be built ({e:#}); the Desk will run without app modules");
            None
        }
    };
    forward_app_env(&project_dir, &package_name, core_wasm.as_deref());

    // `both`: additionally rebuild + hot-swap the app whenever its sources change.
    if args.hot_reload == HotReloadMode::Both {
        spawn_app_watcher(args.port, &project_dir, &package_name, verbose);
    }

    // Launch cargo-leptos watch pipeline (cargo output is kept terse unless -v).
    run_cargo_leptos_watch(&web_cargo)
}

fn run_app_only(args: DevArgs, config: &CliConfig, verbose: bool) -> Result<()> {
    warn!("continuing with `--hot-reload app`");
    super::dev::run(
        DevArgs {
            hot_reload: HotReloadMode::App,
            ..args
        },
        config,
        verbose,
    )
}

/// Env marker used to serve `desk.css` live from the generated stylesheet file.
pub const DESK_CSS_DEV_ENV: &str = "FERRIS_DESK_CSS_DEV";

/// cargo-leptos' fallback when the metadata does not set `reload-port`.
const DEFAULT_RELOAD_PORT: u16 = 3001;

/// Make `--host`/`--port` the address the front-end dev server actually binds.
///
/// cargo-leptos overlays `LEPTOS_*` environment variables on top of
/// `[package.metadata.leptos]`, and the SSR child reads the same
/// `leptos_options.site_addr`, so this is the one place that decides where the
/// Desk is served from.
fn apply_site_addr(web_cargo: &Path, host: &str, port: u16) -> Result<()> {
    let site_addr = site_addr_for(host, port)?;
    let reload_port = reload_port_for(web_cargo, port);

    // SAFETY: single-threaded startup path, before any other thread reads the
    // environment.
    unsafe {
        std::env::set_var("LEPTOS_SITE_ADDR", &site_addr);
        std::env::set_var("LEPTOS_RELOAD_PORT", reload_port.to_string());
    }
    info!(%site_addr, reload_port, "frontend dev server will bind to this address");
    Ok(())
}

/// The `site-addr` to serve from, or an error when it is not a bind address.
fn site_addr_for(host: &str, port: u16) -> Result<String> {
    let site_addr = format!("{host}:{port}");
    if site_addr.parse::<std::net::SocketAddr>().is_err() {
        anyhow::bail!(
            "--host must be an IP address for `--hot-reload front` (got `{host}`); \
             cargo-leptos binds to a socket address, not a hostname"
        );
    }
    Ok(site_addr)
}

/// The live-reload port to use, kept off the site port: cargo-leptos refuses to
/// start when the two are equal.
fn reload_port_for(web_cargo: &Path, site_port: u16) -> u16 {
    let configured = leptos_reload_port(web_cargo).unwrap_or(DEFAULT_RELOAD_PORT);
    if configured == site_port {
        let shifted = site_port.checked_add(1).unwrap_or(DEFAULT_RELOAD_PORT);
        warn!(
            requested = site_port,
            shifted, "reload-port was already taken by the site port; moving it"
        );
        return shifted;
    }
    configured
}

/// `reload-port` from `[package.metadata.leptos]`, if the web crate sets one.
fn leptos_reload_port(web_cargo: &Path) -> Option<u16> {
    let raw = std::fs::read_to_string(web_cargo).ok()?;
    let doc = raw.parse::<toml_edit::DocumentMut>().ok()?;
    doc.get("package")?
        .get("metadata")?
        .get("leptos")?
        .get("reload-port")?
        .as_integer()
        .and_then(|port| u16::try_from(port).ok())
}

/// Loads `ferris-web/Cargo.toml` configuration and hands execution over to `cargo-leptos`.
fn run_cargo_leptos_watch(cargo_toml_path: &Path) -> Result<()> {
    let cargo_toml_path = cargo_toml_path.to_path_buf();

    // This manages WASM compilation (in-memory bindgen), Tailwind CSS builds,
    // WebSocket live-reload, and SSR server lifecycle.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // cargo-leptos feeds its watcher through a broadcast channel of depth 10, and
    // a receiver that falls further behind than that gets `RecvError::Lagged`,
    // which its watch loop turns into a hard error: it requests shutdown, kills
    // the SSR server and returns. A burst of more than ten events while it is
    // busy (a branch switch, a formatter, an editor saving a tree) is enough.
    //
    // There is nothing to keep alive after that — the whole front end lives in
    // cargo-leptos — so the recovery is to run it again. It reads the config from
    // disk each time, which is why a fresh attempt is not a special case.
    watch_until_stable(|| {
        let cli = Cli::parse_from([
            "ferris-dev".as_ref(),
            OsStr::new("--manifest-path"),
            cargo_toml_path.as_os_str(),
            OsStr::new("watch"),
        ]);
        rt.block_on(cargo_leptos::run(cli))
    })
}

/// How many times a failed watch is retried before the error is the caller's.
const WATCH_ATTEMPTS: u32 = 3;

/// Run `attempt` until it succeeds or the attempts run out, reporting the last
/// error. Only the final failure is fatal, and it is the original error.
fn watch_until_stable(mut attempt: impl FnMut() -> Result<()>) -> Result<()> {
    for tries_left in (1..=WATCH_ATTEMPTS).rev() {
        match attempt() {
            Ok(()) => return Ok(()),
            Err(e) if tries_left > 1 => {
                warn!(
                    attempts_left = tries_left - 1,
                    error = %e,
                    "the front-end watch loop stopped; starting it again"
                );
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("the last attempt returns its error")
}

/// Environment marker prefix for the dev SSR server (see `ferris-web/src/bin/ssr.rs`).
const DEV_ENV_PREFIX: &str = "FERRIS_DEV_";

/// Forwards the resolved project configuration to the cargo-leptos SSR child so
/// it joins the same database and uses the same JWT secret as `ferris serve`.
fn forward_dev_env(settings: &ferris_server::ServerSettings) {
    let database = &settings.database;
    let set = |key: &str, value: String| {
        // SAFETY: single-threaded startup path; no other thread reads the env
        // at this point, so mutating the process env cannot race.
        unsafe { std::env::set_var(format!("{DEV_ENV_PREFIX}{key}"), value) };
    };

    set(
        "DB_TYPE",
        match database.r#type {
            DatabaseType::Postgres => "postgres".to_string(),
            DatabaseType::MySQL => "mysql".to_string(),
            DatabaseType::SQLite => "sqlite".to_string(),
        },
    );
    set("DB_HOST", database.host.clone());
    set("DB_PORT", database.port.to_string());
    set("DB_NAME", database.name.clone());
    set("DB_USER", database.user.clone());
    set("DB_PASSWORD", database.password.clone());
    set("DB_MAX_CONNS", database.max_connections.to_string());
    set(
        "DB_SSL_MODE",
        match database.ssl_mode {
            SslMode::Disable => "disable".to_string(),
            SslMode::Allow => "allow".to_string(),
            SslMode::Prefer => "prefer".to_string(),
            SslMode::Require => "require".to_string(),
            SslMode::VerifyCa => "verify_ca".to_string(),
            SslMode::VerifyFull => "verify_full".to_string(),
        },
    );
    set(
        "JWT_SECRET",
        settings.config.auth.jwt_secret.clone().unwrap_or_default(),
    );

    info!("forwarded database + auth config to the frontend dev server");
}

/// Build the app to wasm and return the compiled core module path.
fn build_app_plugin(project_dir: &Path, package_name: &str, verbose: bool) -> Result<PathBuf> {
    super::dev::build_app_wasm(project_dir, package_name, !verbose)?;
    let core_wasm = super::dev::app_core_wasm_path(project_dir, package_name, "dev");
    if !core_wasm.is_file() {
        anyhow::bail!("compiled app wasm not found at {}", core_wasm.display());
    }
    Ok(core_wasm)
}

/// Forward the app plugin location + a reload token to the SSR dev server.
fn forward_app_env(project_dir: &Path, package_name: &str, core_wasm: Option<&Path>) {
    // SAFETY: single-threaded startup path (see `forward_dev_env`).
    unsafe {
        std::env::set_var(
            format!("{DEV_ENV_PREFIX}PROJECT_DIR"),
            project_dir.as_os_str(),
        );
        std::env::set_var(format!("{DEV_ENV_PREFIX}APP_NAME"), package_name);
        std::env::set_var(
            format!("{DEV_ENV_PREFIX}APP_CRATE"),
            package_name.replace('-', "_"),
        );
        std::env::set_var(
            format!("{DEV_ENV_PREFIX}RELOAD_TOKEN"),
            uuid::Uuid::new_v4().to_string(),
        );
        if let Some(path) = core_wasm {
            std::env::set_var(format!("{DEV_ENV_PREFIX}APP_CORE_WASM"), path.as_os_str());
        }
    }
    info!(
        "forwarded app plugin {} to the frontend dev server",
        package_name
    );
}

/// Keep cargo output terse; leave the developer's own cargo settings alone.
///
/// `RUSTFLAGS` must stay exactly as it is (or unset). It is part of cargo's
/// fingerprint, so overriding it here gives the dev server a build graph of its
/// own: every `ferris dev --hot-reload front|both` then recompiles the whole
/// framework — wasmtime and cranelift included — from scratch and leaves a
/// second copy of every artifact in `target/`. On this repository that turned a
/// warm start into a ~15 minute build. `CARGO_TERM_QUIET` has no effect on
/// fingerprints, so it is safe to flip.
fn apply_cargo_quiet(verbose: bool) {
    // SAFETY: startup path; no other thread reads the env at this point.
    unsafe {
        if verbose {
            std::env::remove_var("CARGO_TERM_QUIET");
        } else {
            std::env::set_var("CARGO_TERM_QUIET", "true");
        }
    }
}

/// Rebuild the app and hot-swap it into the running SSR dev server through its
/// loopback reload endpoint whenever the app sources change.
fn spawn_app_watcher(port: u16, project_dir: &Path, package_name: &str, verbose: bool) {
    let project_dir = project_dir.to_path_buf();
    let package_name = package_name.to_string();
    let token = std::env::var(format!("{DEV_ENV_PREFIX}RELOAD_TOKEN")).unwrap_or_default();
    let reload_url = format!("http://127.0.0.1:{port}/__dev/reload-app?token={token}");

    thread::spawn(move || {
        let (tx, rx) = mpsc::channel::<Event>();
        // Dropping the watcher stops the notifications, so it must outlive the loop.
        let _watcher = match super::dev::start_watcher(&project_dir, tx) {
            Ok(w) => w,
            Err(e) => {
                warn!("app watcher could not start: {e:#}");
                return;
            }
        };

        let client = reqwest::blocking::Client::new();
        super::dev::watch_and_reload(
            &rx,
            |event| info!("App change detected: {event:?}"),
            || {
                if let Err(e) = super::dev::build_app_wasm(&project_dir, &package_name, !verbose) {
                    warn!("app rebuild failed: {e:#}");
                    return;
                }
                match client
                    .post(&reload_url)
                    .timeout(Duration::from_secs(120))
                    .send()
                {
                    Ok(resp) if resp.status().is_success() => {
                        info!("app plugin hot-reloaded into the dev server");
                    }
                    Ok(resp) => warn!(
                        status = %resp.status(),
                        "app reload endpoint rejected the request (is the dev server up?)"
                    ),
                    Err(e) => warn!("could not reach the dev server to reload the app: {e}"),
                }
            },
            || super::dev::artifact_is_current(&project_dir, &package_name),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Metadata of a `ferris-web`-shaped crate, with the reload port the real
    /// one leaves unset.
    const WEB_CARGO: &str = r#"
        [package]
        name = "ferris-web"

        [package.metadata.leptos]
        site-addr = "127.0.0.1:8000"
        bin-features = ["ssr"]
    "#;

    fn write(cargo_toml: &str) -> (PathBuf, TempDir) {
        let dir = TempDir::new();
        let path = dir.path().join("Cargo.toml");
        std::fs::write(&path, cargo_toml).unwrap();
        (path, dir)
    }

    /// Minimal scratch directory: the CLI has no temp-dir dependency.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let id = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            let path = std::env::temp_dir()
                .join(format!("ferris-dev-frontend-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_requested_host_and_port_win_over_the_metadata() {
        let (cargo_toml, _dir) = write(WEB_CARGO);
        assert_eq!(leptos_reload_port(&cargo_toml), None);
        assert_eq!(site_addr_for("127.0.0.1", 4321).unwrap(), "127.0.0.1:4321");
        // The site port is not the reload port, so the default still works.
        assert_eq!(reload_port_for(&cargo_toml, 4321), DEFAULT_RELOAD_PORT);
    }

    #[test]
    fn a_configured_reload_port_is_respected() {
        let (cargo_toml, _dir) = write(
            r#"
            [package.metadata.leptos]
            reload-port = 9002
        "#,
        );
        assert_eq!(leptos_reload_port(&cargo_toml), Some(9002));
        assert_eq!(reload_port_for(&cargo_toml, 8000), 9002);
    }

    #[test]
    fn a_reload_port_that_collides_with_the_site_moves() {
        let (cargo_toml, _dir) = write(
            r#"
            [package.metadata.leptos]
            reload-port = 3001
        "#,
        );
        assert_eq!(
            reload_port_for(&cargo_toml, 3001),
            3002,
            "cargo-leptos refuses to start when both ports are equal"
        );
        assert_eq!(site_addr_for("0.0.0.0", 3001).unwrap(), "0.0.0.0:3001");
    }

    #[test]
    fn a_hostname_is_refused_with_an_explanation() {
        let err =
            site_addr_for("localhost", 8000).expect_err("a hostname cannot be a bind address");
        assert!(
            err.to_string().contains("--host must be an IP address"),
            "{err}"
        );
    }

    #[test]
    fn a_watch_loop_that_stops_is_started_again() {
        let runs = std::sync::atomic::AtomicU32::new(0);
        let result = watch_until_stable(|| {
            let run = runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // A `Lagged` the first two times, then it settles.
            if run < 2 {
                anyhow::bail!("recv error: channel lagged");
            }
            Ok(())
        });

        assert!(result.is_ok());
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[test]
    fn a_watch_loop_that_never_starts_gives_up_with_its_own_error() {
        let runs = std::sync::atomic::AtomicU32::new(0);
        let result = watch_until_stable(|| -> Result<()> {
            runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            anyhow::bail!("manifest is not a Cargo.toml")
        });

        assert_eq!(
            runs.load(std::sync::atomic::Ordering::SeqCst),
            WATCH_ATTEMPTS
        );
        let err = result.expect_err("a watch that never starts must fail");
        assert!(
            err.to_string().contains("manifest is not a Cargo.toml"),
            "the caller gets the real error, not a retry notice: {err}"
        );
    }

    #[test]
    fn a_watch_loop_that_stops_cleanly_is_not_retried() {
        let runs = std::sync::atomic::AtomicU32::new(0);
        watch_until_stable(|| {
            runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
        .unwrap();

        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
