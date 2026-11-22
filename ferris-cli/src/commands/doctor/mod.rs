//! Doctor command - check project health
//!
//! Checks are aligned with the `ferris new` scaffold so that a freshly
//! generated project passes cleanly: `src/` + `config/` + `Cargo.toml` are
//! critical; migrations, WIT and external services (DB server, Redis) are
//! reported without failing the run.

mod checks;
mod features;
mod health;
mod migrations;
mod report;
mod run;

pub use run::run;

#[cfg(test)]
mod tests;
