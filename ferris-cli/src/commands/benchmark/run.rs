//! Entry point of `ferris benchmark`.

use anyhow::Result;
use tracing::{info, warn};

use crate::commands::{BenchmarkArgs, BenchmarkWhat};
use crate::config::CliConfig;
use crate::project;

use super::concurrency::clamp_concurrency;
use super::scenarios::{
    benchmark_api, benchmark_database, benchmark_jobs, benchmark_print, benchmark_realtime,
};

pub fn run(args: BenchmarkArgs, config: &CliConfig) -> Result<()> {
    project::check_project_exists(&config.source.project_dir)?;

    let iterations = args.iterations.max(1);
    let (concurrency, note) = clamp_concurrency(args.concurrency);
    if let Some(note) = note {
        warn!("{note}");
    }
    info!(
        "Benchmark starting (iterations={iterations}, concurrency={concurrency}): {:?}",
        args.what
    );

    match args.what {
        BenchmarkWhat::All => {
            benchmark_api(&args, config)?;
            benchmark_database(&args, config, concurrency)?;
            benchmark_realtime(&args, config)?;
            benchmark_jobs(&args, config)?;
            benchmark_print(&args, config)?;
        }
        BenchmarkWhat::Api => benchmark_api(&args, config)?,
        BenchmarkWhat::Database => {
            benchmark_database(&args, config, concurrency)?;
        }
        BenchmarkWhat::Realtime => benchmark_realtime(&args, config)?,
        BenchmarkWhat::Jobs => benchmark_jobs(&args, config)?,
        BenchmarkWhat::Print => benchmark_print(&args, config)?,
    }

    Ok(())
}
