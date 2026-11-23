//! Entry point of `ferris doctor`.

use anyhow::Result;
use tracing::info;

use crate::config::CliConfig;

use super::checks::{
    check_cargo, check_cargo_toml, check_config_files, check_database, check_locales,
    check_project_structure, check_redis, check_rust_toolchain, check_secret_fields,
    check_wasm_target, check_wit_version,
};
use super::features::check_features;
use super::migrations::check_migrations;
use super::report::print_results;

pub fn run(config: &CliConfig) -> Result<()> {
    info!("Running Ferris health check...");
    let cwd = std::env::current_dir()?;

    let bench_root = config.source.bench_root.as_path();
    let checks = vec![
        check_rust_toolchain(),
        check_cargo(),
        check_cargo_toml(&cwd, bench_root),
        check_project_structure(&cwd, bench_root),
        check_config_files(&config.source.config_path),
        check_wit_version(&cwd, bench_root),
        check_locales(bench_root),
        // Before any network check: a config that asks for a subsystem this
        // binary does not contain can never boot, whatever the database says.
        check_features(config),
        check_database(config),
        check_redis(config),
        check_secret_fields(config),
        check_migrations(&cwd),
        check_wasm_target(),
    ];

    print_results(&checks, &cwd);

    let failed = checks.iter().filter(|c| !c.passed && c.critical).count();
    if failed > 0 {
        return Err(anyhow::anyhow!("{failed} critical checks failed"));
    }

    Ok(())
}
