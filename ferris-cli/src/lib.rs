//! Ferris CLI - library surface.
//!
//! The command modules and argument types are exposed so the CLI can be driven
//! programmatically (and reused by the `ferris` binary).

pub mod commands;
pub mod config;
pub mod context;
pub mod features;
pub mod framework;
pub mod project;
pub mod runtime;

pub use commands::{
    BenchmarkArgs, BenchmarkWhat, BuildArgs, Commands, ConfigArgs, ConfigCommand, DeployArgs,
    DeployTarget, DevArgs, GenerateArgs, GenerateWhat, MigrateArgs, NewArgs, ShellArgs, TenantArgs,
    TenantCommand,
};
pub use config::CliConfig;
