//! The argument tree of `ferris site`.
//!
//! Declared next to the implementation (`commands/site/`) rather than in
//! `commands/mod.rs`: that file holds the top-level `Commands` enum and the
//! arguments of the commands that do not own a module, and it grew past the
//! 600-line limit the moment the site tree was added to it.

use clap::{Parser, Subcommand};

/// `ferris site`: the `sites/<site>/` layout (Frappe's unit of deployment).
#[derive(Parser)]
pub struct SiteArgs {
    #[command(subcommand)]
    pub command: SiteCommand,
}

#[derive(Subcommand)]
pub enum SiteCommand {
    /// Create a site: its layout, database and admin user
    New {
        /// Directory name under `sites/`, and the name `--site` takes
        name: String,
        /// The `Host` this site answers for (default: the site name)
        #[arg(long, value_name = "HOST")]
        host: Option<String>,
        /// Database name (default: the site name with non-identifier characters replaced)
        #[arg(long, value_name = "NAME")]
        db_name: Option<String>,
        /// Database type: postgres, mysql or sqlite (default: `[database] type`)
        #[arg(long, value_name = "TYPE")]
        db_type: Option<String>,
        /// Email of the site's System Manager
        #[arg(long, value_name = "EMAIL")]
        admin_email: String,
        /// Admin password. Prefer `FERRIS_ADMIN_PASSWORD`: a flag is this
        /// process's argv, so `ps` shows it to every user on the machine.
        #[arg(long, env = "FERRIS_ADMIN_PASSWORD", hide_env_values = true)]
        admin_password: String,
    },
    /// List the sites of this project
    List,
    /// Show one site's configuration
    Info { site: String },
    /// Delete a site's configuration (its database and files are kept)
    Delete {
        site: String,
        /// Skip the confirmation prompt. Still only removes the configuration:
        /// data removal is out of scope and needs the database's own tooling.
        #[arg(long)]
        force: bool,
    },
    /// Enable a site so `--site` may serve it
    Enable { site: String },
    /// Disable a site without touching its data
    Disable { site: String },
    /// Apply the migrations to a site's own database
    Migrate {
        site: String,
        #[arg(long)]
        target: Option<u32>,
    },
    /// Install an app from the global catalogue into one site
    InstallApp {
        app: String,
        #[arg(long, value_name = "NAME")]
        site: String,
        /// Upgrade an already installed app, keeping the site's data and secrets
        #[arg(long)]
        force: bool,
    },
    /// Remove an app from one site
    UninstallApp {
        app: String,
        #[arg(long, value_name = "NAME")]
        site: String,
        /// Delete the instance too (its data, files and secrets)
        #[arg(long)]
        purge: bool,
    },
}
