//! One runtime for the blocking setup work a plugin load has to do.
//!
//! Loading a plugin is synchronous by contract ([`crate::PluginStore::swap_load`]
//! runs on a watcher thread, and the SSR reload endpoint parks on a plain
//! thread), but the host services it wires up are async: the Redis handshake in
//! [`crate::host_impl`], the naming-series and `ensure_table` migrations in
//! [`crate::store`]. Each of those used to build a fresh
//! `tokio::runtime::Runtime` — a full multi-thread pool, twelve worker threads on
//! a twelve-core box — on every single load, and `Runtime::block_on` panics when
//! the calling thread is already driving a runtime.
//!
//! One **multi-thread** runtime, created once per process, serves all of it.
//!
//! It cannot be current-thread. sqlx returns a pooled connection from a task it
//! spawns on the ambient runtime (`sqlx_core::pool::connection`), and a
//! current-thread runtime is only polled while `block_on` is driving the setup
//! future: the return task is queued after that future completes and never
//! runs, so every plugin load permanently loses one connection. With
//! `max_connections = 1` the next load is `PoolExhausted`; with the production
//! default the leak is slower but just as real across hot reloads. Keeping two
//! worker threads alive is enough to drain those tasks, and still far cheaper
//! than the fresh twelve-thread runtime each load used to build.

use std::future::Future;
use std::sync::OnceLock;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ferris-plugin-setup")
            .enable_all()
            .build()
            .expect("multi-thread runtime for plugin setup")
    })
}

/// Run a setup future to completion.
///
/// Never panics for being called from inside a runtime: when the caller is
/// already on one, the future moves to a helper thread that drives the shared
/// runtime instead. Callers are synchronous by contract, so parking a worker
/// while a helper thread waits is the same trade `swap_load` already makes by
/// compiling the component inline.
pub fn block_on<F>(fut: F) -> F::Output
where
    F: Future + Send,
    F::Output: Send,
{
    let runtime = runtime();
    if tokio::runtime::Handle::try_current().is_err() {
        return runtime.block_on(fut);
    }

    std::thread::scope(|scope| {
        scope
            .spawn(|| runtime.block_on(fut))
            .join()
            .expect("plugin setup thread")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferris_db::Stmt;

    /// A one-connection pool must survive repeated setup calls: the task that
    /// returns each connection to sqlx is spawned on the ambient runtime, so a
    /// runtime that stops being polled after `block_on` leaks one connection per
    /// call (the original `PoolExhausted` in `certificate_service`).
    #[tokio::test(flavor = "multi_thread")]
    async fn repeated_setup_returns_every_pooled_connection() {
        let db = ferris_db::memory_db().await.expect("in-memory database");
        for _ in 0..4 {
            let db = db.clone();
            block_on(async move {
                db.fetch_optional(&Stmt::new("SELECT 1", vec![]))
                    .await
                    .expect("setup query");
            });
        }
        db.health_check()
            .await
            .expect("the pool still has its only connection");
    }
}
