//! One function per benchmark target.

use anyhow::{Result, anyhow};
use std::time::Instant;
use tracing::info;

use crate::commands::BenchmarkArgs;
use crate::commands::db_map::core_database;
use crate::config::CliConfig;

use super::concurrency::run_id;
use super::outcome::Outcome;
use super::redis::check_redis_is_writable;

fn health_url(config: &CliConfig) -> String {
    std::env::var("FERRIS_BENCH_URL").unwrap_or_else(|_| {
        format!(
            "http://{}:{}/api/health",
            config.server.host, config.server.port
        )
    })
}

pub(super) fn benchmark_api(args: &BenchmarkArgs, config: &CliConfig) -> Result<()> {
    let url = health_url(config);
    info!("Benchmarking API: {url}");
    let client = reqwest::blocking::Client::new();

    // Reachability used to be `probe.is_err()`, i.e. "the TCP connection and the
    // HTTP framing worked". A 404 from a server that is up but does not serve
    // /api/health passed the probe, and then every iteration failed while the
    // report looked like a measurement.
    match client.get(&url).send() {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => {
            return Err(anyhow!(
                "{url} answered {} — that is not a healthy endpoint to measure. \
                 Start the server with the API role (`ferris serve --api`) or set \
                 FERRIS_BENCH_URL.",
                response.status()
            ));
        }
        Err(e) => {
            return Err(anyhow!(
                "cannot reach {url}: {e}. Start the server first (`ferris serve --api`)."
            ));
        }
    }
    for _ in 0..5 {
        let _ = client.get(&url).send();
    }

    let iterations = args.iterations.max(1);
    let mut outcome = Outcome::default();
    for _ in 0..iterations {
        let start = Instant::now();
        let result = client
            .get(&url)
            .send()
            .map_err(|e| e.to_string())
            .and_then(|response| {
                if response.status().is_success() {
                    Ok(response)
                } else {
                    Err(format!("{}", response.status()))
                }
            });
        outcome.record(start.elapsed(), result.map(|_| ()));
    }

    outcome.report("API")
}

pub(super) fn benchmark_database(
    args: &BenchmarkArgs,
    config: &CliConfig,
    concurrency: usize,
) -> Result<()> {
    let mut database = core_database(&config.database, config.database.name.clone())?;
    info!(
        "Benchmarking database ({:?} at {}:{}, {concurrency} workers)",
        database.r#type, config.database.host, config.database.port
    );

    let iterations = args.iterations.max(1);
    database.max_connections = database.max_connections.max(concurrency as u32);

    // The only benchmark that spawns anything: one task per worker.
    let rt = crate::runtime::block_on_runtime(Some(concurrency))?;

    rt.block_on(async {
        let db = std::sync::Arc::new(ferris_db::create_pool(&database).await?);
        let outcome = std::sync::Arc::new(std::sync::Mutex::new(Outcome::default()));

        let per_worker = iterations.div_ceil(concurrency);
        let workers: Vec<_> = (0..concurrency)
            .map(|_| {
                let db = db.clone();
                let outcome = outcome.clone();
                rt.spawn(async move {
                    for _ in 0..per_worker {
                        let start = Instant::now();
                        let result = db.health_check().await.map_err(|e| e.to_string());
                        outcome
                            .lock()
                            .unwrap()
                            .record(start.elapsed(), result.map(|_| ()));
                    }
                })
            })
            .collect();

        for worker in workers {
            worker.await.map_err(anyhow::Error::msg)?;
        }

        let outcome = std::mem::take(&mut *outcome.lock().unwrap());
        outcome.report("Database (SELECT 1)")
    })
}

pub(super) fn benchmark_realtime(args: &BenchmarkArgs, config: &CliConfig) -> Result<()> {
    let url = config.redis.url.clone();
    let iterations = args.iterations.max(1);
    check_redis_is_writable(
        &url,
        args.allow_remote_write,
        "realtime",
        &format!("publishes {iterations} messages"),
    )?;

    // A channel of this run's own: `ferris:bench` reached whatever was already
    // subscribed, which on a shared instance means real handlers.
    let channel = format!("ferris:bench-{}", run_id());
    info!("Benchmarking realtime transport (Redis pub/sub) at {url}");
    info!("  publishing to {channel} (transient, nothing is written to disk)");

    let client =
        redis::Client::open(url.as_str()).map_err(|e| anyhow!("invalid redis URL {url}: {e}"))?;
    let mut conn = client
        .get_connection()
        .map_err(|e| anyhow!("cannot connect to Redis at {url}: {e}"))?;

    let mut outcome = Outcome::default();
    for i in 0..iterations {
        let start = Instant::now();
        let published = redis::cmd("PUBLISH")
            .arg(&channel)
            .arg(i.to_string())
            .query::<i64>(&mut conn)
            .map_err(|e| e.to_string());
        outcome.record(start.elapsed(), published);
    }

    outcome.report("Realtime (publish)")
}

pub(super) fn benchmark_jobs(args: &BenchmarkArgs, config: &CliConfig) -> Result<()> {
    let url = config.redis.url.clone();
    let iterations = args.iterations.max(1);
    check_redis_is_writable(
        &url,
        args.allow_remote_write,
        "jobs",
        &format!("enqueues {iterations} durable jobs"),
    )?;

    // `JobQueue::enqueue` writes a sorted set called `queue:<name>`, so a queue
    // named for this run is both something nobody else can be using and
    // something this run can delete afterwards.
    let queue_name = format!("bench-{}", run_id());
    info!("Benchmarking job queue enqueue at {url}");
    info!("  enqueuing into {queue_name} and deleting it afterwards");

    let rt = crate::runtime::block_on_runtime(None)?;

    rt.block_on(async {
        let client = redis::Client::open(url.as_str())
            .map_err(|e| anyhow!("invalid redis URL {url}: {e}"))?;
        let queue = ferris_jobs::JobQueue::new(
            ferris_jobs::QueueConfig {
                name: queue_name.clone(),
                ..Default::default()
            },
            client.clone(),
        )
        .await?;

        let mut outcome = Outcome::default();
        for i in 0..iterations {
            let payload = ferris_core::Value::from(serde_json::json!({"n": i}));
            let job = ferris_jobs::Job::new(format!("bench-{i}"), payload);
            let start = Instant::now();
            let enqueued = queue.enqueue(job).await.map_err(|e| e.to_string());
            outcome.record(start.elapsed(), enqueued.map(|_| ()));
        }

        // These jobs are durable and nothing will ever run them, so they are
        // removed instead of left for the next person to wonder about.
        let key = format!("queue:{queue_name}");
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| anyhow!("cannot connect to Redis at {url}: {e}"))?;
        let removed: usize = redis::cmd("DEL")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap_or(0);
        if removed > 0 {
            info!("  deleted {key}");
        }

        outcome.report("Jobs (enqueue)")
    })
}

/// A build without `print` cannot measure Typst. Say so instead of silently
/// dropping the scenario from `--what all`.
#[cfg(not(feature = "print"))]
pub(super) fn benchmark_print(_args: &BenchmarkArgs, _config: &CliConfig) -> Result<()> {
    Err(anyhow!(
        "the print benchmark requires a build with `--features print`"
    ))
}

#[cfg(feature = "print")]
pub(super) fn benchmark_print(args: &BenchmarkArgs, _config: &CliConfig) -> Result<()> {
    info!("Benchmarking print (Typst PDF via PrintEngine)");

    let (invoice_stem, invoice_name, invoice_src) = ferris_print::BUILTIN_TEMPLATES[0];
    info!("  template: {invoice_name} ({invoice_stem}.typ)");

    let engine_config = ferris_print::PrintConfig {
        template_dir: None,
        font_dir: None,
        ..Default::default()
    };

    let rt = crate::runtime::block_on_runtime(None)?;

    rt.block_on(async {
        let engine = ferris_print::PrintEngine::new(engine_config)?;
        engine.register_format(ferris_print::PrintFormatDef::new(
            invoice_stem.to_string(),
            invoice_name.to_string(),
            invoice_src.to_string(),
        ));

        let data = ferris_core::Value::from(serde_json::json!({
            "customer": {
                "name": "Benchmark Customer",
                "email": "bench@example.com",
                "address": "123 Test St",
            },
            "vendor": {
                "name": "Ferris",
                "address": "Main St",
            },
            "invoice_number": "INV-BENCH-0001",
            "date": "2026-09-25",
            "items": [
                {"description": "Benchmark line item", "qty": 1, "rate": 10.0}
            ],
        }));

        let iterations = args.iterations.max(1);
        let mut outcome = Outcome::default();
        let mut largest: u64 = 0;
        let mut total: u64 = 0;
        for _ in 0..iterations {
            let start = Instant::now();
            let result = engine
                .render(invoice_stem, &data, &ferris_print::PrintOptions::default())
                .map(|out| {
                    // The report used to keep the *last* render's size and call it
                    // "output size", so 999 renders' worth of output vanished and
                    // one run that varied reported whatever happened to be last.
                    let size = out.len() as u64;
                    total += size;
                    largest = largest.max(size);
                })
                .map_err(|e| e.to_string());
            outcome.record(start.elapsed(), result);
        }

        let renders = outcome.succeeded as u64;
        outcome.report("Print (PDF)")?;
        let average = if renders == 0 { 0 } else { total / renders };
        println!("  output size:   {average} bytes per PDF (largest {largest}, {total} total)");
        Ok(())
    })
}
