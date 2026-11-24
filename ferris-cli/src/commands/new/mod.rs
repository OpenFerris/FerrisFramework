//! New project command: scaffold a WIT/WASM component app.

mod cargo_files;
mod command;
mod config_files;
mod git;
mod lib_rs;
mod project_docs;
mod structure;
mod toolchain;

pub use command::run;
pub(in crate::commands) use config_files::write_bench_config;
pub(in crate::commands) use project_docs::write_bench_gitignore;

#[cfg(test)]
mod tests;
