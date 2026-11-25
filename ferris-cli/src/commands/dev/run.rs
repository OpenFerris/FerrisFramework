//! Entry point of `ferris dev`.

use anyhow::Result;
use ferris_server::{ServerRole, ServerRoles};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tracing::{error, info};

use crate::commands::dev_css::{self, DESK_CSS_DEV_ENV};
use crate::commands::plugin_boot;
use crate::commands::{DevArgs, HotReloadMode, serve};
use crate::config::CliConfig;

use super::artifact::artifact_is_current;
use super::hydrate::ensure_hydrate_bundle;
use super::project::{build_app_wasm, check_ferris_project, load_plugin, rebuild_and_reload};
use super::seed::seed_dev_admin;
use super::watch::{start_watcher, watch_and_reload};

pub fn run(args: DevArgs, config: &CliConfig, verbose: bool) -> Result<()> {
    // Development shares `serve`/`run` auth settings; `dev_config` only
    // clones them and the dev profile relaxes the cookie to the plain name.
    let mut config = super::dev_config(config);

    // A site rewrites the process-wide data locations before anything reads
    // them, and installs its `Host` allow-list. It is applied *before* the
    // frontend handoff so the supervisor's `settings_for` and the child that
    // re-enters this same function both serve the site, not the project.
    if let Some(name) = args.site.as_deref() {
        crate::commands::site::apply_site_by_name(&mut config, name)?;
    }

    // Resolve the app before anything reads the working directory: starting at
    // the bench root (`ferris dev --app billing`, or the only app in `apps/`)
    // moves to the app directory once, so the build, watcher, hydrate bundle
    // and frontend supervisor all see the same crate.
    let app_dir = crate::commands::build::resolve_app_dir(&config, args.app.as_deref())?;
    let invocation = std::env::current_dir()?;
    if invocation != app_dir {
        std::env::set_current_dir(&app_dir)?;
    }
    config.source.project_dir = app_dir.clone();
    // The catalogue is a bench-level path; pin it before `plugins_dir` joins a
    // relative `wasm_dir` inside the app. A `--site` already set an absolute
    // instance root, which must win.
    if args.site.is_none() && config.apps.wasm_dir.is_relative() {
        config.apps.wasm_dir = config.source.bench_root.join(&config.apps.wasm_dir);
    }

    // `--hot-reload front` turns this process into a supervisor: the desk
    // (ferris-web) is rebuilt on change and the dev server is restarted as a
    // child, because SSR code lives inside the server binary.
    //
    // `dev_frontend` only re-enters this function with `hot_reload = App` (see
    // its `run_app_only` fallback), so there is no recursion to guard against.
    if matches!(args.hot_reload, HotReloadMode::Front | HotReloadMode::Both) {
        return crate::commands::dev_frontend::run(args, &config, verbose);
    }

    // 1. Verify we're in a ferris project
    let project_dir = std::env::current_dir()?;
    check_ferris_project(&project_dir)?;

    // 2. Read package name from Cargo.toml
    let package_name = crate::commands::build::package_name(&project_dir)?;
    info!("Detected app: {}", package_name);

    // 3. Build the app to a wasm component
    build_app_wasm(&project_dir, &package_name, !verbose)?;

    // 3b. Make sure the hydrate wasm bundle is present under ./pkg so the web
    // role serves it at /pkg. It is read from disk (the framework checkout, the
    // cargo-leptos output or `FERRIS_HYDRATE_DIR`); when none of them exists the
    // Desk still renders server-side and the call warns once instead of failing.
    ensure_hydrate_bundle(&project_dir)?;

    // 4. Prepare server settings (roles + database from project config)
    let roles = ServerRoles::parse(&[
        ServerRole::Api.as_str().to_string(),
        ServerRole::Web.as_str().to_string(),
        ServerRole::Realtime.as_str().to_string(),
    ])?;
    let mut settings = serve::settings_for(args.host.clone(), args.port, roles, &config)?;

    // 5. One runtime for the whole startup path: the pool, the admin seeding and
    // the plugin load all need it, and building one per call would spawn sets of
    // worker threads only to drop them again. It must be multi-threaded:
    // `plugin_boot::load` parks a worker with `block_in_place`, which a
    // current-thread runtime rejects.
    let runtime_handle = Arc::new(tokio::runtime::Runtime::new()?);
    let pool = runtime_handle.block_on(ferris_db::create_pool(&settings.database))?;

    // 5b. Seed a default operator so the Desk login works out of the box in
    // dev. No-op when the users table already has an account.
    seed_dev_admin(&runtime_handle, &pool)?;

    // Store policy (compile cache, epoch interrupts) lives in `plugin_boot`, so
    // dev and production instantiate the same runtime the same way.
    let plugins = plugin_boot::new_store(
        pool.clone(),
        Some(&plugin_boot::plugins_dir(&project_dir, &config.apps)),
    )?;

    // 6. Load the initial component, registering its DocTypes + lifecycle
    // events as part of publishing it.
    load_plugin(
        &runtime_handle,
        &plugins,
        &project_dir,
        &package_name,
        &config.apps,
    )?;

    // 7. Set up file watcher for hot-reload
    let (tx, rx) = mpsc::channel();
    let _watcher = start_watcher(&project_dir, tx)?;

    // Store host/port for browser opening (before move)
    let host = args.host.clone();
    let port = args.port;

    settings.plugins = Some(plugins.clone());
    settings.pool = Some(pool);

    // The desk bundle is rewritten in place while this server runs, so `/pkg`
    // must not be cached by the browser (see `ServerSettings::dev`).
    settings.dev = true;

    // Serve `desk.css` live from the generated stylesheet file so theme edits
    // apply without recompiling the framework. The stylesheet is compiled before
    // the server starts, because the server reads this setting once, at boot.
    if config.cli.unlock_source {
        if let Some(css) =
            css_dev_path().or_else(|| std::env::var_os(DESK_CSS_DEV_ENV).map(PathBuf::from))
        {
            info!(css = %css.display(), "Desk stylesheet served from disk");
            settings.desk_css_dev = Some(css);
        }
    }

    // 8. Spawn hot-reload task (std thread, since we're not in tokio runtime yet)
    let reload_runtime = runtime_handle.clone();
    // The watcher outlives `run`, so it needs its own copy of the grant
    // decision: a reload must re-apply the operator's `[apps]` policy, not the
    // one that happened to be around when the thread was spawned.
    let reload_apps = config.apps.clone();
    thread::spawn(move || {
        watch_and_reload(
            &rx,
            |event| info!("Change detected: {event:?}"),
            || {
                if let Err(e) = rebuild_and_reload(
                    &reload_runtime,
                    &plugins,
                    &project_dir,
                    &package_name,
                    &reload_apps,
                    verbose,
                ) {
                    error!("Hot-reload failed: {}", e);
                } else {
                    info!("Hot-reload successful");
                }
            },
            || artifact_is_current(&project_dir, &package_name),
        );
    });

    // 9. Open browser if requested
    if args.open {
        let url = format!("http://{}:{}", host, port);
        thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1200));
            info!("opening {}", url);
            let _ = open::that(&url);
        });
    }

    info!("Starting development server on {}:{}", host, port);
    info!(database = %settings.database.name, "using shared project database");
    serve::start(settings, &config)
}

/// Start the live stylesheet and report where it is written.
///
/// `None` when the framework sources cannot be located or Tailwind is not
/// installed: the server then serves the stylesheet baked into the binary, the
/// same as before. Neither is worth failing a dev server over.
fn css_dev_path() -> Option<PathBuf> {
    let framework = crate::framework::framework_root().ok()?;
    dev_css::spawn(&framework.join("ferris-web"))
}
