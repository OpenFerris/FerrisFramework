//! CLI commands

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

pub mod app_catalogues;

#[cfg(feature = "wasm")]
pub mod app;
pub mod backup;
pub mod benchmark;
#[cfg(feature = "wasm")]
pub mod build;
pub mod config;
pub(crate) mod db_map;
#[cfg(feature = "wasm")]
pub mod deploy;
#[cfg(feature = "wasm")]
pub mod dev;
pub mod dev_css;
#[cfg(feature = "wasm")]
pub mod dev_frontend;
pub mod doctor;
pub mod generate;
pub mod info;
pub mod migrate;
pub mod multi_site;
pub mod new;
#[cfg(feature = "wasm")]
pub mod plugin_boot;
pub mod plugin_world;
pub mod role_permissions;
pub mod run;
pub mod serve;
pub mod shell;
pub mod site;
pub mod tenant;

// The site subcommand tree lives with its module (`site/args.rs`): this file
// declares the command surface, not each command's whole argument list.
pub use site::{SiteArgs, SiteCommand};

#[derive(Subcommand)]
pub enum Commands {
    /// Create a new Ferris project
    New(NewArgs),

    /// Start the development server (API + web + realtime)
    #[cfg(feature = "wasm")]
    Dev(DevArgs),

    /// Serve the framework on a single port, choosing roles with flags
    Serve(ServeArgs),

    /// Serve using the roles in `server.roles` from the config
    Run(RunArgs),

    /// Build the project
    #[cfg(feature = "wasm")]
    Build(BuildArgs),

    /// Install, list, enable or remove WASM apps in this project
    #[cfg(feature = "wasm")]
    App(AppArgs),

    /// Run database migrations
    Migrate(MigrateArgs),

    /// Back up the SQLite database, files and config into a timestamped directory
    Backup(BackupArgs),

    /// Restore a backup created by `ferris backup`
    Restore(RestoreArgs),

    /// Generate code from schema
    Generate(GenerateArgs),

    /// Deploy the application
    #[cfg(feature = "wasm")]
    Deploy(DeployArgs),

    /// Manage tenants
    Tenant(TenantArgs),

    /// Manage sites: independent deployments under `sites/<site>/`
    Site(SiteArgs),

    /// Start interactive shell
    Shell(ShellArgs),

    /// Run benchmarks
    Benchmark(BenchmarkArgs),

    /// Show project info
    Info,

    /// Check project health
    Doctor,

    /// Manage configuration
    Config(ConfigArgs),

    /// Manage Role Permissions (DocPerm rows) in the configured database
    RolePermissions(RolePermissionsArgs),
}

#[derive(Parser)]
pub struct NewArgs {
    /// Project name
    pub name: String,

    /// Directory to create the app in (default: `<bench>/apps/<name>`).
    ///
    /// Not `--dir`: that is the global working directory, and two flags of the
    /// same name meaning different things is how `ferris new -d x app` and
    /// `ferris --dir x new app` came to disagree.
    #[arg(long, value_name = "DIR")]
    pub into: Option<String>,

    /// Create a standalone app with its own `config/ferris.toml` and the
    /// historical `plugins/` output, instead of the `apps/` + `sites/` bench.
    #[arg(long)]
    pub standalone: bool,
}

#[derive(Parser)]
pub struct DevArgs {
    /// Port to run on
    #[arg(short, long, default_value = "8000")]
    pub port: u16,

    /// App to run when starting from the bench root (default: the only app
    /// under `apps/`, or the app directory the command runs in).
    #[arg(long, value_name = "NAME")]
    pub app: Option<String>,

    /// Host to bind to
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,

    /// Open the desk in the browser
    #[arg(long)]
    pub open: bool,

    /// Hot-reload scope
    #[arg(long, value_enum, default_value = "app", value_name = "MODE")]
    pub hot_reload: HotReloadMode,

    /// Serve a site from `sites/<name>/`: its database, files root, Redis,
    /// enabled apps and plugin instances. One process per site.
    #[arg(long, value_name = "NAME")]
    pub site: Option<String>,
}

/// How much of the stack `ferris dev` watches and reloads.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HotReloadMode {
    /// Rebuild + swap the app WASM plugin when its sources change (default).
    #[default]
    App,
    /// Hot-reload the desk frontend (`ferris-web`): Tailwind CSS is rebuilt
    /// and served live, the hydrate WASM bundle is regenerated and, because
    /// SSR runs inside the server process, the dev server itself is
    /// recompiled and restarted — like `cargo leptos watch` with SSR. The app
    /// plugin is still loaded once so the Desk shows its modules.
    #[value(name = "front", alias = "frontend")]
    Front,
    /// Hot-reload the desk frontend AND the app at the same time: the app
    /// WASM plugin is rebuilt and swapped into the running dev server when
    /// its sources change, in addition to the full frontend pipeline.
    Both,
}

#[derive(Parser)]
pub struct ServeArgs {
    /// Host to bind to
    #[arg(long, default_value = "0.0.0.0")]
    pub host: String,

    /// Port to bind to
    #[arg(short, long, default_value = "8000")]
    pub port: u16,

    /// Serve the REST API (`/api/*`)
    #[arg(long)]
    pub api: bool,

    /// Serve the desk web UI
    #[arg(long)]
    pub web: bool,

    /// Serve realtime (socket.io on the shared port)
    #[arg(long)]
    pub realtime: bool,

    /// Register background workers (email queue, WIT apps)
    #[arg(long)]
    pub worker: bool,

    /// Register the scheduler (background jobs / cron-like tasks)
    #[arg(long)]
    pub scheduler: bool,

    /// Enable every role
    #[arg(long)]
    pub all: bool,

    /// Allow this origin for CORS (repeatable). Passing any origin also
    /// enables CORS; there is no wildcard mode.
    #[arg(long = "cors-origin")]
    pub cors_origin: Vec<String>,

    /// Serve an instance for this site: apply its database, files root, Redis
    /// URL, enabled apps and plugin instances before boot, and answer only its
    /// `host_name` (the rest get `421`). The isolation model is one process per
    /// site; requests are not routed by `Host`.
    #[arg(long, value_name = "NAME", conflicts_with = "tenant")]
    pub site: Option<String>,

    /// Serve an instance for this tenant (the pre-sites registry). Prefer
    /// `--site`: the sites/ layout also carries the app instances.
    #[arg(long, value_name = "ID")]
    pub tenant: Option<String>,

    /// Serve every enabled site from this one address, routing by `Host`.
    ///
    /// One process per site, one public port. Each site keeps its own
    /// database, files root, Redis and schema registry; a `Host` with no site
    /// answers `421`.
    #[arg(long, conflicts_with_all = ["site", "tenant"])]
    pub all_sites: bool,
}

#[derive(Parser)]
pub struct RunArgs {
    /// Serve an instance for this site, like `ferris serve --site`.
    #[arg(long, value_name = "NAME", conflicts_with = "tenant")]
    pub site: Option<String>,

    /// Serve an instance for this tenant, like `ferris serve --tenant`.
    /// Prefer `--site`.
    #[arg(long, value_name = "ID")]
    pub tenant: Option<String>,
}

#[derive(Parser)]
pub struct BuildArgs {
    /// Build profile
    #[arg(short, long, default_value = "release")]
    pub profile: String,

    /// Output directory
    #[arg(short, long)]
    pub out_dir: Option<String>,

    /// App to build when the command runs from the bench root.
    #[arg(long, value_name = "NAME")]
    pub app: Option<String>,
}

#[derive(Parser)]
pub struct MigrateArgs {
    /// Target migration version
    #[arg(short, long)]
    pub target: Option<u32>,

    /// Dry run
    #[arg(long)]
    pub dry_run: bool,

    /// Force migration
    #[arg(long)]
    pub force: bool,
}

#[derive(Parser)]
pub struct BackupArgs {
    /// Directory to write the timestamped backup into (default: `./backups`)
    #[arg(long, value_name = "DIR", conflicts_with = "verify")]
    pub out: Option<PathBuf>,

    /// Verify an existing backup directory instead of creating one
    #[arg(long, value_name = "DIR")]
    pub verify: Option<PathBuf>,
}

#[derive(Parser)]
pub struct RestoreArgs {
    /// Backup directory to restore from
    #[arg(long, value_name = "DIR")]
    pub from: PathBuf,

    /// Skip the confirmation prompt. Required when stdin is not a terminal.
    #[arg(long)]
    pub yes: bool,
}

#[derive(Parser)]
pub struct GenerateArgs {
    /// Type of code to generate
    #[arg(short, long, value_enum, default_value = "all")]
    pub what: GenerateWhat,

    /// Output directory
    #[arg(short, long)]
    pub out_dir: Option<String>,

    /// Force overwrite
    #[arg(long)]
    pub force: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum GenerateWhat {
    All,
    Schema,
    Api,
    Models,
    Routes,
    Typescript,
    Wit,
    Templates,
}

#[derive(Parser)]
pub struct DeployArgs {
    /// Deployment target
    #[arg(short, long, value_enum, default_value = "docker")]
    pub target: DeployTarget,

    /// Environment
    #[arg(short, long, default_value = "production")]
    pub env: String,

    /// Skip build
    #[arg(long)]
    pub skip_build: bool,

    /// Directory to deploy into, for `--target binary` (default `/opt/ferris/<env>`)
    #[arg(long)]
    pub to: Option<String>,

    /// Registry to tag and push to, for `--target docker` (default: build locally only)
    #[arg(long)]
    pub registry: Option<String>,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum DeployTarget {
    Docker,
    Kubernetes,
    Serverless,
    Binary,
}

#[derive(Parser)]
pub struct TenantArgs {
    #[command(subcommand)]
    pub command: TenantCommand,
}

#[derive(Subcommand)]
pub enum TenantCommand {
    /// Create a new tenant
    Create {
        name: String,
        host: String,
        #[arg(long)]
        db_name: Option<String>,
        #[arg(long)]
        admin_email: String,
        /// Admin password. Visible to anyone running `ps`, so prefer
        /// `FERRIS_ADMIN_PASSWORD` or letting the command ask for it.
        #[arg(long)]
        admin_password: Option<String>,
    },
    /// List all tenants
    List,
    /// Get tenant info
    Info { id: String },
    /// Delete a tenant's configuration (its database and files are kept)
    Delete {
        id: String,
        /// Skip the confirmation prompt. Still only removes the configuration:
        /// data removal is out of scope and needs the database's own tooling.
        #[arg(long)]
        force: bool,
    },
    /// Run migrations for tenant
    Migrate {
        id: String,
        #[arg(long)]
        target: Option<u32>,
    },
    /// Enable tenant
    Enable { id: String },
    /// Disable tenant
    Disable { id: String },
}

#[derive(Parser)]
pub struct ShellArgs {
    /// A `ferris` command to run and exit, e.g. `ferris shell doctor`.
    ///
    /// Not `last = true`: there are no other flags here, so requiring `--`
    /// between the shell and its command only made `ferris shell info` a parse
    /// error.
    pub command: Vec<String>,
}

#[derive(Parser)]
pub struct BenchmarkArgs {
    /// Benchmark to run
    #[arg(short, long, value_enum, default_value = "all")]
    pub what: BenchmarkWhat,

    /// Number of iterations
    #[arg(short, long, default_value = "1000")]
    pub iterations: usize,

    /// Concurrent workers (threads and database connections)
    #[arg(short = 'n', long, default_value_t = benchmark::DEFAULT_CONCURRENCY)]
    pub concurrency: usize,

    /// Allow the `realtime` and `jobs` benchmarks to write to a Redis that is not
    /// this machine (they publish messages and enqueue durable jobs there)
    #[arg(long)]
    pub allow_remote_write: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum BenchmarkWhat {
    All,
    Api,
    Database,
    Realtime,
    Jobs,
    Print,
}

#[derive(Parser)]
pub struct AppArgs {
    #[command(subcommand)]
    pub command: AppCommand,
}

#[derive(Subcommand)]
pub enum AppCommand {
    /// List the global app catalogue and, with `--site`, the apps installed in
    /// that site (or, on an older install, the components in `[apps] wasm_dir`)
    List {
        /// Also list the apps installed in this site
        #[arg(long, value_name = "NAME")]
        site: Option<String>,
    },
    /// Install a package (a directory with `plugin.toml`, `<app>.wasm` and
    /// `world/`) or a component into the global catalogue, or copy a component
    /// into `[apps] wasm_dir` on an install that predates the catalogue
    Install {
        /// Path to the package directory or the `.wasm` component
        path: PathBuf,
        /// Publish it under this name (default: the file stem)
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove an app from the global catalogue (or from `[apps] wasm_dir` on an
    /// older install); the world is kept unless `--purge` is given
    Remove {
        name: String,
        /// Delete the app world too (data, files and secrets)
        #[arg(long)]
        purge: bool,
    },
    /// Add an app to `[apps] enabled`
    Enable { name: String },
    /// Remove an app from `[apps] enabled`
    Disable { name: String },
}

#[derive(Parser)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Parser)]
pub struct RolePermissionsArgs {
    #[command(subcommand)]
    pub command: RolePermissionsCommand,
}

#[derive(Subcommand)]
pub enum RolePermissionsCommand {
    /// List role permission (DocPerm) rows, optionally for one DocType
    List {
        /// The DocType to list, or every row when omitted
        doctype: Option<String>,
    },
    /// Create or overwrite a role's rights on a DocType
    ///
    /// The row is written to the configured database; a running server reads
    /// it on its next restart. Use `/api/authz/role-permissions` (the desk's
    /// Role Permissions Builder) for a live reload.
    Set {
        doctype: String,
        role: String,
        /// Permission level. Rows above 0 are stored but not translated into
        /// engine statements, because field-level checks do not exist yet.
        #[arg(long, default_value_t = 0)]
        permlevel: u8,
        #[arg(long)]
        read: bool,
        #[arg(long)]
        write: bool,
        #[arg(long)]
        create: bool,
        #[arg(long)]
        delete: bool,
        #[arg(long)]
        submit: bool,
        #[arg(long)]
        cancel: bool,
        #[arg(long)]
        amend: bool,
        #[arg(long)]
        print: bool,
        #[arg(long)]
        email: bool,
        #[arg(long)]
        share: bool,
        #[arg(long)]
        report: bool,
        #[arg(long)]
        import: bool,
        #[arg(long)]
        export: bool,
        /// Grant the rights to the document's owner only
        #[arg(long)]
        if_owner: bool,
        /// Apply the rights to every DocType, not just `doctype`
        #[arg(long)]
        all_doctypes: bool,
    },
    /// Delete every row for a `(doctype, role)` pair, across permission levels
    Delete { doctype: String, role: String },
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Show current configuration
    Show,
    /// Set a configuration value
    Set { key: String, value: String },
    /// Get a configuration value
    Get { key: String },
    /// Reset configuration
    Reset,
    /// Edit configuration in editor
    Edit,
}
