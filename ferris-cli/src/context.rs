//! Where the CLI is running: a bench root, a site, an app, or nothing yet.
//!
//! Frappe's bench is a directory with one `apps/` and one `sites/` tree, and the
//! command you need depends on where you stand inside it. Ferris follows the
//! same model: `ferris init` creates the bench, `ferris new` fills `apps/`,
//! `ferris site new` fills `sites/`, and the commands that operate on an
//! instance only make sense from the bench root, from that site or from an app.
//!
//! The zone is computed from the working directory before clap parses, so
//! `ferris --help` lists what can actually run here and an out-of-place command
//! fails with the zone and the alternatives instead of a confusing path error.
//!
//! A **standalone** project (the pre-bench layout: `config/ferris.toml` plus the
//! crate at the root) is an app zone with its own bench root, so `ferris dev`
//! and `ferris build` keep working exactly as before.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Command;

use crate::commands::{Commands, SiteCommand};
use crate::config::CliConfig;

/// The part of the tree the command was invoked from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Zone {
    /// A bench root: `config/ferris.toml`, `apps/` and/or `sites/`.
    Bench { root: PathBuf },
    /// `sites/<name>/` inside a bench.
    Site {
        bench: PathBuf,
        name: String,
        dir: PathBuf,
    },
    /// `apps/<name>/` inside a bench, or a standalone app project.
    App {
        bench: PathBuf,
        name: String,
        dir: PathBuf,
    },
    /// No Ferris project above the working directory.
    Outside,
}

/// The directory the CLI should treat as its working directory.
///
/// `--dir` is a global flag and may appear before or after the subcommand, so
/// the zone has to be resolved from the raw arguments *before* clap restricts
/// the command list. Without this, `ferris --dir sites/acme --help` advertises
/// the commands of the caller's directory, not the site's. `cwd` is the
/// fallback when the flag is absent.
#[must_use]
pub fn invocation_dir<I, S>(args: I, cwd: PathBuf) -> PathBuf
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy();
        if text == "--" {
            break;
        }
        if text == "--dir" || text == "-d" {
            if let Some(value) = args.next() {
                return PathBuf::from(value);
            }
            break;
        }
        if let Some(value) = text.strip_prefix("--dir=") {
            if !value.is_empty() {
                return PathBuf::from(value);
            }
            break;
        }
        if let Some(value) = text.strip_prefix("-d") {
            let value = value.strip_prefix('=').unwrap_or(value);
            if !value.is_empty() {
                return PathBuf::from(value);
            }
        }
    }
    cwd
}

/// Detect the zone from `cwd`.
///
/// The nearest enclosing site or app wins over the bench it lives in, so
/// `apps/billing/src` is an app and not a bench.
#[must_use]
pub fn detect(cwd: &Path) -> Zone {
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());

    if let Some((site, dir)) = nearest_site(&cwd) {
        return Zone::Site {
            bench: bench_of(&dir, "sites"),
            name: site,
            dir,
        };
    }
    if let Some((app, dir)) = nearest_app(&cwd) {
        return Zone::App {
            bench: bench_of(&dir, "apps"),
            name: app,
            dir,
        };
    }
    if let Some((root, _)) = crate::config::find_bench_root(&cwd) {
        // A bench owns `apps/` and `sites/`; a project with only a config is
        // the standalone layout, whose root is the app itself.
        if root.join("apps").is_dir() || root.join("sites").is_dir() {
            return Zone::Bench { root };
        }
        let name = app_name(&root);
        return Zone::App {
            bench: root.clone(),
            name,
            dir: root,
        };
    }
    Zone::Outside
}

/// The best site name for messages and implicit `--site`.
#[must_use]
pub fn site_name(zone: &Zone) -> Option<&str> {
    match zone {
        Zone::Site { name, .. } => Some(name),
        _ => None,
    }
}

/// A short description for errors and help.
#[must_use]
pub fn label(zone: &Zone) -> String {
    match zone {
        Zone::Bench { root } => format!("at the bench root `{}`", root.display()),
        Zone::Site { name, .. } => format!("inside site `{name}`"),
        Zone::App { name, .. } => format!("inside app `{name}`"),
        Zone::Outside => "outside a Ferris bench (run `ferris init` first)".to_string(),
    }
}

/// Top-level commands that make sense in `zone`, in help order.
///
/// Commands compiled out of this binary (`dev`, `build`, `app`, `deploy` need
/// the `wasm` feature) are dropped so help and errors never name a command that
/// cannot run here.
#[must_use]
pub fn allowed(zone: &Zone) -> Vec<&'static str> {
    catalogue(zone)
        .iter()
        .copied()
        .filter(|name| compiled_in(name))
        .collect()
}

fn compiled_in(name: &str) -> bool {
    #[cfg(feature = "wasm")]
    {
        let _ = name;
        true
    }
    #[cfg(not(feature = "wasm"))]
    {
        !matches!(name, "dev" | "build" | "app" | "deploy")
    }
}

fn catalogue(zone: &Zone) -> &'static [&'static str] {
    match zone {
        Zone::Bench { .. } => &[
            "new",
            "dev",
            "site",
            "app",
            "build",
            "serve",
            "run",
            "migrate",
            "backup",
            "restore",
            "deploy",
            "generate",
            "tenant",
            "shell",
            "benchmark",
            "role-permissions",
            "info",
            "doctor",
            "config",
        ],
        Zone::Site { .. } => &[
            "serve",
            "run",
            "migrate",
            "backup",
            "restore",
            "site",
            "shell",
            "benchmark",
            "role-permissions",
            "info",
            "doctor",
            "config",
        ],
        Zone::App { .. } => &[
            "dev", "build", "generate", "shell", "info", "doctor", "config",
        ],
        Zone::Outside => &["init", "new", "info", "doctor", "config"],
    }
}

/// Hide the subcommands `allowed` does not list, so `--help` matches the zone.
pub fn restrict(command: Command, zone: &Zone) -> Command {
    let allowed = allowed(zone);
    let names: Vec<String> = command
        .get_subcommands()
        .map(|sub| sub.get_name().to_string())
        .collect();
    let mut command = command;
    for name in names {
        if !allowed.contains(&name.as_str()) {
            command = command.mut_subcommand(name, hide);
        }
    }
    command
}

fn hide(sub: Command) -> Command {
    sub.hide(true)
}

/// Reject a command the zone does not allow, naming what does work.
///
/// # Errors
///
/// The command is not available in this zone, `ferris new` was asked for
/// outside a bench without `--standalone`, or a `ferris site` subcommand needs
/// the bench root.
pub fn check(zone: &Zone, command: &Commands) -> Result<()> {
    if matches!(command, Commands::Init(_)) && !matches!(zone, Zone::Outside) {
        bail!(
            "this directory already belongs to a bench; `ferris init` fills an empty folder, while here you can use: {}",
            allowed(zone).join(", ")
        );
    }

    let name = command_name(command);
    if !allowed(zone).contains(&name) {
        return Err(not_allowed(zone, name));
    }

    #[cfg(feature = "wasm")]
    if let Commands::Dev(args) = command
        && matches!(zone, Zone::Bench { .. })
        && args.app.is_none()
    {
        bail!(
            "`ferris dev` at the bench root needs `--app <name>`; or run it from `apps/<name>` and it picks the app itself"
        );
    }

    if let Commands::New(args) = command
        && matches!(zone, Zone::Outside)
        && !args.standalone
    {
        bail!(
            "there is no bench here: run `ferris init` to create one, or \
             `ferris new {} --standalone` for a single-app project",
            args.name
        );
    }

    if let Commands::Site(args) = command {
        let sub = site_command_name(&args.command);
        if !site_subcommand_allowed(zone, sub) {
            bail!(
                "`ferris site {sub}` is not available {}; run it from the bench root.\n\
                 Inside a site you can use: {}",
                label(zone),
                site_subcommands(zone)
            );
        }
    }

    Ok(())
}

/// Fill the site a command is about from the zone, and point the process at
/// that site's database before the command opens it.
///
/// Only the commands that take a `--site`/site name get the default; `serve`,
/// `run` and `dev` keep doing their own rewrite, which also installs the Host
/// allow-list.
///
/// # Errors
///
/// Applying the enclosing site fails (an unreadable `site_config.json`, a
/// disabled site, a duplicate host).
pub fn prepare(zone: &Zone, command: &mut Commands, config: &mut CliConfig) -> Result<()> {
    let Zone::Site { name, .. } = zone else {
        return Ok(());
    };

    match command {
        Commands::Serve(args) => {
            args.site.get_or_insert_with(|| name.clone());
        }
        Commands::Run(args) => {
            args.site.get_or_insert_with(|| name.clone());
        }
        #[cfg(feature = "wasm")]
        Commands::Dev(args) => {
            args.site.get_or_insert_with(|| name.clone());
        }
        // These open the configured pool directly; rewrite the config first so
        // they hit the site's database rather than the bench one.
        Commands::Migrate(_)
        | Commands::Backup(_)
        | Commands::Restore(_)
        | Commands::Shell(_)
        | Commands::RolePermissions(_)
        | Commands::Doctor
        | Commands::Info
        | Commands::Config(_) => {
            crate::commands::site::apply_site_by_name(config, name)?;
        }
        Commands::Site(args) => match &mut args.command {
            SiteCommand::Info { site }
            | SiteCommand::Enable { site }
            | SiteCommand::Disable { site }
            | SiteCommand::Migrate { site, .. }
            | SiteCommand::InstallApp { site, .. }
            | SiteCommand::UninstallApp { site, .. } => {
                site.get_or_insert_with(|| name.clone());
            }
            SiteCommand::New { .. } | SiteCommand::List | SiteCommand::Delete { .. } => {}
        },
        _ => {}
    }
    Ok(())
}

fn not_allowed(zone: &Zone, name: &str) -> anyhow::Error {
    let text = format!(
        "`ferris {name}` is not available {}.\nAvailable here: {}.\n\
         Run `ferris --help` for the commands of this directory.",
        label(zone),
        allowed(zone).join(", ")
    );
    anyhow::anyhow!(text)
}

fn site_subcommand_allowed(zone: &Zone, sub: &str) -> bool {
    match zone {
        Zone::Bench { .. } => true,
        Zone::Site { .. } => matches!(
            sub,
            "info" | "enable" | "disable" | "migrate" | "install-app" | "uninstall-app"
        ),
        _ => false,
    }
}

fn site_subcommands(zone: &Zone) -> &'static str {
    match zone {
        Zone::Site { .. } => "info, enable, disable, migrate, install-app, uninstall-app",
        _ => "new, list, info, delete, enable, disable, migrate, install-app, uninstall-app",
    }
}

fn command_name(command: &Commands) -> &'static str {
    match command {
        Commands::Init(_) => "init",
        Commands::New(_) => "new",
        #[cfg(feature = "wasm")]
        Commands::Dev(_) => "dev",
        Commands::Serve(_) => "serve",
        Commands::Run(_) => "run",
        #[cfg(feature = "wasm")]
        Commands::Build(_) => "build",
        #[cfg(feature = "wasm")]
        Commands::App(_) => "app",
        Commands::Migrate(_) => "migrate",
        Commands::Backup(_) => "backup",
        Commands::Restore(_) => "restore",
        Commands::Generate(_) => "generate",
        #[cfg(feature = "wasm")]
        Commands::Deploy(_) => "deploy",
        Commands::Tenant(_) => "tenant",
        Commands::Site(_) => "site",
        Commands::Shell(_) => "shell",
        Commands::Benchmark(_) => "benchmark",
        Commands::Info => "info",
        Commands::Doctor => "doctor",
        Commands::Config(_) => "config",
        Commands::RolePermissions(_) => "role-permissions",
    }
}

fn site_command_name(command: &SiteCommand) -> &'static str {
    match command {
        SiteCommand::New { .. } => "new",
        SiteCommand::List => "list",
        SiteCommand::Info { .. } => "info",
        SiteCommand::Delete { .. } => "delete",
        SiteCommand::Enable { .. } => "enable",
        SiteCommand::Disable { .. } => "disable",
        SiteCommand::Migrate { .. } => "migrate",
        SiteCommand::InstallApp { .. } => "install-app",
        SiteCommand::UninstallApp { .. } => "uninstall-app",
    }
}

fn nearest_site(start: &Path) -> Option<(String, PathBuf)> {
    for dir in start.ancestors() {
        if dir.join("site_config.json").is_file()
            && dir
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "sites")
        {
            return Some((file_name(dir), dir.to_path_buf()));
        }
    }
    None
}

fn nearest_app(start: &Path) -> Option<(String, PathBuf)> {
    for dir in start.ancestors() {
        let is_app_dir = dir.join("plugin.toml").is_file() || dir.join("Cargo.toml").is_file();
        if is_app_dir
            && dir
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "apps")
        {
            return Some((file_name(dir), dir.to_path_buf()));
        }
    }
    None
}

fn bench_of(dir: &Path, parent_name: &str) -> PathBuf {
    dir.parent()
        .filter(|parent| parent.file_name().is_some_and(|name| name == parent_name))
        .and_then(Path::parent)
        .map_or_else(|| dir.to_path_buf(), Path::to_path_buf)
}

fn app_name(root: &Path) -> String {
    let cargo = root.join("Cargo.toml");
    if let Ok(text) = std::fs::read_to_string(cargo)
        && let Some(name) = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("name = "))
    {
        let name = name.trim().trim_matches('"');
        if !name.is_empty() {
            return name.to_string();
        }
    }
    file_name(root)
}

fn file_name(dir: &Path) -> String {
    dir.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ferris-zone-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("the scratch root is writable");
        root
    }

    fn bench(root: &Path) {
        std::fs::create_dir_all(root.join("config")).expect("config");
        std::fs::create_dir_all(root.join("apps")).expect("apps");
        std::fs::create_dir_all(root.join("sites")).expect("sites");
        std::fs::write(root.join("config/ferris.toml"), "").expect("bench config");
    }

    #[test]
    fn a_folder_without_a_config_is_outside() {
        let root = scratch("outside");
        assert_eq!(detect(&root), Zone::Outside);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_bench_root_and_its_subdirectories_are_the_bench() {
        let root = scratch("bench");
        bench(&root);
        assert_eq!(
            detect(&root),
            Zone::Bench {
                root: root.canonicalize().expect("canonical")
            }
        );
        let config = root.join("config");
        assert!(matches!(detect(&config), Zone::Bench { .. }));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_site_directory_wins_over_the_bench() {
        let root = scratch("site");
        bench(&root);
        let site = root.join("sites/acme");
        std::fs::create_dir_all(&site).expect("site");
        std::fs::write(site.join("site_config.json"), "{}").expect("site config");
        std::fs::create_dir_all(site.join("private")).expect("private");

        let zone = detect(&site.join("private"));
        match zone {
            Zone::Site { name, dir, .. } => {
                assert_eq!(name, "acme");
                assert_eq!(dir, site.canonicalize().expect("canonical"));
            }
            other => panic!("expected a site zone, got {other:?}"),
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_app_directory_wins_over_the_bench() {
        let root = scratch("app");
        bench(&root);
        let app = root.join("apps/billing");
        std::fs::create_dir_all(app.join("src")).expect("app");
        std::fs::write(app.join("Cargo.toml"), "name = \"billing\"\n").expect("cargo");

        let zone = detect(&app.join("src"));
        match zone {
            Zone::App { name, dir, bench } => {
                assert_eq!(name, "billing");
                assert_eq!(dir, app.canonicalize().expect("canonical"));
                assert_eq!(bench, root.canonicalize().expect("canonical"));
            }
            other => panic!("expected an app zone, got {other:?}"),
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_standalone_project_is_an_app_zone() {
        let root = scratch("standalone");
        std::fs::create_dir_all(root.join("config")).expect("config");
        std::fs::write(root.join("config/ferris.toml"), "").expect("config file");
        std::fs::write(root.join("Cargo.toml"), "name = \"solo\"\n").expect("cargo");

        match detect(&root) {
            Zone::App { name, bench, .. } => {
                assert_eq!(name, "solo");
                assert_eq!(bench, root.canonicalize().expect("canonical"));
            }
            other => panic!("expected the standalone app zone, got {other:?}"),
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn each_zone_advertises_a_different_set() {
        assert!(
            allowed(&Zone::Bench {
                root: PathBuf::new()
            })
            .contains(&"site")
        );
        // `dev` is listed at the bench root but `check` demands `--app` there.
        assert!(
            allowed(&Zone::Bench {
                root: PathBuf::new()
            })
            .contains(&"dev")
        );
        assert!(allowed(&Zone::Outside).contains(&"init"));

        let site = Zone::Site {
            bench: PathBuf::new(),
            name: "acme".into(),
            dir: PathBuf::new(),
        };
        assert!(site_subcommand_allowed(&site, "install-app"));
        assert!(!site_subcommand_allowed(&site, "new"));
        assert!(!site_subcommand_allowed(&site, "delete"));
    }

    #[test]
    fn the_dir_flag_wins_over_the_process_directory() {
        let cwd = PathBuf::from("/tmp/caller");
        assert_eq!(
            invocation_dir(["--dir", "/bench/sites/acme"], cwd.clone()),
            PathBuf::from("/bench/sites/acme")
        );
        assert_eq!(
            invocation_dir(["--dir=/bench", "site", "info"], cwd.clone()),
            PathBuf::from("/bench")
        );
        assert_eq!(
            invocation_dir(["site", "info", "-d", "/bench"], cwd.clone()),
            PathBuf::from("/bench")
        );
        assert_eq!(
            invocation_dir(["site", "info"], cwd.clone()),
            cwd,
            "without --dir the process directory is the zone"
        );
    }
}
