//! Realtime shutdown wiring for the socket.io engine.

use socketioxide::SocketIo;

/// Close socket.io connections when the process begins draining.
///
/// The realtime layer keeps upgraded HTTP connections open, and axum's graceful
/// shutdown waits for every connection task. `SocketIo::close()` sends the
/// closing handshake and drops those sockets, so the drain does not depend on
/// the browser tab being closed first.
pub(super) fn watch_realtime_shutdown(io: SocketIo) {
    // `build_router` is synchronous and is also called from unit tests; a
    // caller without a runtime simply does not get the watcher.
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let mut shutdown = ferris_api::shutdown::subscribe();
    tokio::spawn(async move {
        while !*shutdown.borrow() {
            if shutdown.changed().await.is_err() {
                return;
            }
        }
        io.close().await;
    });
}
