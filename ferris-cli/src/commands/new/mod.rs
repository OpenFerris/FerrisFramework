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

#[cfg(test)]
mod tests;
