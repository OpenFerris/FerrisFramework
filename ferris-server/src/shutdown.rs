//! Signal handling and the bounded drain around `axum::serve`.
//!
//! `axum::serve(..).with_graceful_shutdown(..)` stops accepting new
//! connections and then waits for every in-flight connection to finish. That
//! is correct for ordinary requests, but some transports are deliberately
//! long-lived: the `/events` SSE bridge, `/api/notifications/stream`, socket.io
//! upgrades and plugin `WebSockets`. A browser keeps those open indefinitely, so
//! without a second signal the wait never ends and a container stop looks hung.
//!
//! Two mechanisms cooperate:
//!
//! * [`ferris_api::shutdown`] is triggered as soon as SIGINT/SIGTERM arrives;
//!   the stream handlers watch it and complete their response body.
//! * [`serve_with_drain`] gives the server a hard deadline anyway. If a guest
//!   or a custom handler ignores the signal, dropping the serve future stops
//!   the listener and the process does not wait forever.
//!
//! The deadline is deliberately a constant rather than a request timeout: it
//! applies once to the whole process, not to every request, and a deployment
//! that changes `request_timeout_secs` should not accidentally make shutdown
//! unbounded.

use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::watch;
use tracing::{info, warn};

/// How long the listener may keep draining after SIGINT/SIGTERM.
///
/// Long enough to let an ordinary request finish, short enough that a stuck
/// plugin call cannot keep a supervisor waiting forever. The worker pool has its
/// own 30 s job-drain budget; this deadline is for HTTP connections only.
pub(crate) const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Resolve on SIGINT or SIGTERM, then notify the long-lived streams and the
/// drain watcher.
///
/// Without this future `axum::serve` only returns on a fatal error, which made
/// the epoch-clock abort, the worker-pool drain and the scheduler shutdown
/// unreachable in normal operation (SRV5). The extra `started` channel is what
/// lets [`serve_with_drain`] distinguish "still serving" from "signal received,
/// now wait at most `DRAIN_TIMEOUT`".
pub(crate) async fn shutdown_signal(started: watch::Sender<bool>) {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            warn!("failed to listen for ctrl-c: {e}");
            // Never resolve: a listener we cannot install must not turn into an
            // immediate shutdown.
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(e) => {
                warn!("failed to listen for SIGTERM: {e}");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }

    info!("shutdown signal received; closing long-lived streams and draining");
    // Wake SSE/WebSocket handlers first, so the ordinary HTTP drain can finish
    // instead of immediately falling through to the deadline.
    ferris_api::shutdown::trigger();
    let _ = started.send(true);
}

/// Await the server, but stop waiting `drain_timeout` after a shutdown signal.
///
/// A future that completes normally returns its result. Once `shutdown_started`
/// changes, the server gets `drain_timeout` to finish in-flight requests. On
/// timeout the serve future is dropped and the caller proceeds to the background
/// role shutdown; the Tokio runtime then tears down any remaining connection
/// tasks.
pub(crate) async fn serve_with_drain<F>(
    serve: F,
    shutdown_started: &mut watch::Receiver<bool>,
    drain_timeout: Duration,
) -> Result<()>
where
    F: Future<Output = std::io::Result<()>> + Send,
{
    tokio::pin!(serve);

    tokio::select! {
        result = serve.as_mut() => server_result(result),
        changed = shutdown_started.changed() => {
            // The sender is normally alive for the whole serve call. If it is
            // gone, just await the server; there is no signal to time out on.
            if changed.is_err() || !*shutdown_started.borrow() {
                return server_result(serve.as_mut().await);
            }

            tokio::time::timeout(drain_timeout, serve.as_mut())
                .await
                .map_or_else(
                    |_| {
                        warn!(
                            timeout_secs = drain_timeout.as_secs(),
                            "graceful shutdown timed out; dropping remaining connections"
                        );
                        Ok(())
                    },
                    server_result,
                )
        }
    }
}

fn server_result(result: std::io::Result<()>) -> Result<()> {
    result.map_err(|e| anyhow::anyhow!("server error: {e}"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::watch;

    use super::serve_with_drain;

    #[tokio::test]
    async fn a_stuck_connection_does_not_hold_shutdown_forever() {
        let (tx, mut rx) = watch::channel(false);
        let serve = std::future::pending::<std::io::Result<()>>();
        let task = tokio::spawn(async move {
            serve_with_drain(serve, &mut rx, Duration::from_millis(25)).await
        });

        tx.send(true).expect("the drain receiver is alive");
        let result = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("the drain deadline must wake the task")
            .expect("the drain task must not panic");

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn a_finished_server_returns_without_a_signal() {
        let (_tx, mut rx) = watch::channel(false);
        let result = serve_with_drain(async { Ok(()) }, &mut rx, Duration::from_secs(1)).await;
        assert!(result.is_ok());
    }
}
