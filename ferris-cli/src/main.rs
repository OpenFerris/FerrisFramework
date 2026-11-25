//! Ferris CLI - Command line interface for the Ferris framework

use clap::{CommandFactory, FromArgMatches, Parser};
use color_eyre::Result;
use tracing::error;
use tracing_subscriber::util::SubscriberInitExt;

use ferris_cli::commands;
use ferris_cli::commands::*;
use ferris_cli::config::CliConfig;

/// Ferris CLI - Metadata-driven full-stack web framework
#[derive(Parser)]
#[command(
    name = "ferris",
    version,
    about = "Ferris - Metadata-driven, full-stack web framework in Rust",
    long_about = "Ferris is a high-performance, type-safe web framework built in Rust.\nIt provides a complete stack for building business applications.",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Configuration file path
    #[arg(short, long, global = true)]
    config: Option<String>,

    /// Working directory
    #[arg(short, long, global = true)]
    dir: Option<String>,
}

fn main() -> Result<()> {
    color_eyre::install()?;

    // The zone decides which commands exist here: `ferris --help` inside a
    // site, an app or an uninitialised folder lists a different set, and a
    // command from another zone fails before any config or pool is opened.
    // `--dir` may point at a site or an app while the process still runs from
    // somewhere else; the zone must follow the flag, not the caller's cwd.
    let invocation =
        ferris_cli::context::invocation_dir(std::env::args_os().skip(1), std::env::current_dir()?);
    let zone = ferris_cli::context::detect(&invocation);
    let command = Cli::command();
    let command = ferris_cli::context::restrict(command, &zone);
    let matches = command.get_matches();
    let mut cli = Cli::from_arg_matches(&matches)?;
    if let Err(error) = ferris_cli::context::check(&zone, &cli.command) {
        return Err(color_eyre::eyre::eyre!("{error}"));
    }

    // Setup logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(format!("ferris={},{}", log_level, "warn"))
        .finish();

    // `--hot-reload front`/`both` hands the process over to cargo-leptos, which
    // installs its own global `log` logger (flexi_logger). Skip the tracing→log
    // bridge on that path, otherwise flexi_logger panics with SetLoggerError.
    #[cfg(feature = "wasm")]
    let frontend_handoff = matches!(
        &cli.command,
        Commands::Dev(args)
            if matches!(args.hot_reload, HotReloadMode::Front | HotReloadMode::Both)
    );
    #[cfg(not(feature = "wasm"))]
    let frontend_handoff = false;
    if frontend_handoff {
        tracing::subscriber::set_global_default(subscriber)?;
    } else {
        subscriber.try_init()?;
    }

    // Load configuration
    let mut config = CliConfig::load(cli.config.as_deref(), cli.dir.as_deref())?;

    // Install the `Secret` keyring (H3) as early as possible: `shell`, `migrate`
    // and the server's hooks all reach credentials through the document engine,
    // so the first reader should not be the thing that discovers a bad key.
    // Failing here is deliberate — a malformed `[secrets] key` means nothing
    // written this run could ever be decrypted again.
    ferris_core::secret::install_keyring_from_config(&config.secrets)?;

    // `--dir` means "work in this project", and most commands name paths
    // relative to it (`Cargo.toml`, `plugins/`, `target/`). Moving here once,
    // here, is what makes `--dir` mean anything at all for them.
    let project_dir = config.source.project_dir.clone();
    if !project_dir.is_dir() {
        error!("--dir {} is not a directory", project_dir.display());
        std::process::exit(1);
    }
    std::env::set_current_dir(&project_dir)?;

    // An enclosing site supplies the `--site` of serve/run/dev/migrate/..., and
    // points the config at its database before the command opens it.
    if let Err(error) = ferris_cli::context::prepare(&zone, &mut cli.command, &mut config) {
        return Err(color_eyre::eyre::eyre!("{error}"));
    }

    // Execute command
    let result = match cli.command {
        Commands::Init(args) => commands::init::run(args, &config),
        Commands::New(args) => commands::new::run(args, &config),
        #[cfg(feature = "wasm")]
        Commands::Dev(args) => commands::dev::run(args, &config, cli.verbose),
        Commands::Serve(args) => commands::serve::run(args, &config),
        Commands::Run(args) => commands::run::run(args, &config),
        #[cfg(feature = "wasm")]
        Commands::Build(args) => commands::build::run(args, &config),
        #[cfg(feature = "wasm")]
        Commands::App(args) => commands::app::run(args, &config),
        Commands::Migrate(args) => commands::migrate::run(args, &config),
        Commands::Backup(args) => commands::backup::run(args, &config),
        Commands::Restore(args) => commands::backup::restore(args, &config),
        Commands::Generate(args) => commands::generate::run(args, &config),
        #[cfg(feature = "wasm")]
        Commands::Deploy(args) => commands::deploy::run(args, &config),
        Commands::Tenant(args) => commands::tenant::run(args, &config),
        Commands::Site(args) => commands::site::run(args, &config),
        Commands::Shell(args) => commands::shell::run(args, &config),
        Commands::Benchmark(args) => commands::benchmark::run(args, &config),
        Commands::Info => commands::info::run(&config),
        Commands::Doctor => commands::doctor::run(&config),
        Commands::Config(args) => commands::config::run(args, &config),
        Commands::RolePermissions(args) => commands::role_permissions::run(args, &config),
    };

    if let Err(e) = result {
        error!("Command failed: {e:?}");
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod workflow_macro_tests {
    use ferris_macro::workflow;

    #[workflow(
        name = "Order Approval",
        doctype = "Sales Order",
        initial_state = "Draft",
        states = [
            { name = "Draft", doc_status = "draft", allow_edit = ["Sales User"] },
            { name = "Approved", doc_status = "submitted" },
        ],
        transitions = [
            {
                name = "Approve",
                from = "Draft",
                to = "Approved",
                action = "Approve",
                allowed_roles = ["Sales Manager"]
            },
        ]
    )]
    struct SalesOrderWorkflow;

    #[test]
    fn test_workflow_macro_registers_and_enforces() {
        SalesOrderWorkflow::register_workflow().expect("workflow registers");

        let engine = ferris_document::global_workflow_engine();
        assert!(engine.get("Sales Order").is_some());

        // Role-gated: a plain user cannot approve.
        assert!(
            engine
                .apply(
                    "Sales Order",
                    "Draft",
                    "Approve",
                    &["Sales User".to_string()],
                    &std::collections::HashMap::new(),
                )
                .is_err()
        );

        let outcome = engine
            .apply(
                "Sales Order",
                "Draft",
                "Approve",
                &["Sales Manager".to_string()],
                &std::collections::HashMap::new(),
            )
            .expect("manager can approve");
        assert_eq!(outcome.state, "Approved");
        assert_eq!(outcome.doc_status, ferris_core::DocStatus::Submitted);
    }
}
