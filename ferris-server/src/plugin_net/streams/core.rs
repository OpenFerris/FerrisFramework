//! The stream table and the pump that drives one entry.
//!
//! Every task of the transport shares one [`TransportCore`]: the pump, the
//! accept loop and the public handle address the same entry table and the same
//! store, without a self-referential struct.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use ferris_wasm::{PluginStore, StreamEvent, StreamInfo};
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

use super::super::run_guest;
use super::io::{ChunkSink, ChunkSource};
use super::{CLOSE_GRACE, Limits, WRITE_QUEUE};

/// One stream or listener registered under its id.
pub(super) struct Entry {
    /// The id the guest knows it by; the entry carries it so teardown can
    /// report without the caller remembering which map key it came from.
    pub(super) id: String,
    pub(super) app: String,
    /// Present for a stream: the queue the guest writes into. Dropping it (which
    /// is what `close-stream` does) is the pump's signal to flush and close.
    pub(super) sender: Option<mpsc::Sender<Vec<u8>>>,
    /// The pump (stream) or accept loop (listener); `None` for the instant
    /// between inserting the entry and having the task in hand.
    pub(super) abort: Option<AbortHandle>,
    /// Signalled by the pump once the wire is closed, so `close-stream` can wait
    /// for a graceful close instead of aborting mid-frame.
    pub(super) done: Option<oneshot::Receiver<()>>,
    /// The listener that accepted this connection, when the app owns one.
    /// `close-stream` on the listener tears down exactly these.
    pub(super) parent: Option<String>,
}

/// The state every task of the transport shares.
///
/// A single `Arc` of this is what makes the ownership work: the pump task, the
/// accept loop and the public handle all address the same entry table and the
/// same store, without a self-referential struct.
pub(super) struct TransportCore {
    pub(super) store: Arc<PluginStore>,
    pub(super) entries: Mutex<HashMap<String, Entry>>,
}

impl TransportCore {
    /// Tell the guest one stream exists.
    fn announce(&self, app: &str, id: &str, info: StreamInfo) {
        if let Err(e) = self.store.register_stream(app, id, info) {
            tracing::warn!(app = %app, stream = %id, error = %e, "stream registration failed");
        }
    }

    /// Emit one lifecycle event (fire-and-await: `opened` must be ordered before
    /// the first chunk, so this is awaited).
    async fn event(&self, app: &str, id: &str, event: StreamEvent) -> Result<()> {
        let store = Arc::clone(&self.store);
        let app = app.to_string();
        let id = id.to_string();
        run_guest(move || {
            store
                .handle_stream_event(&app, &id, event)
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
        .await
    }

    /// Forward one chunk and return the reply.
    async fn chunk(&self, app: &str, id: &str, bytes: Vec<u8>) -> Result<Vec<u8>> {
        let store = Arc::clone(&self.store);
        let app = app.to_string();
        let id = id.to_string();
        run_guest(move || {
            store
                .handle_stream(&app, &id, &bytes)
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
        .await
    }

    /// Forget `id`, release the guest's per-stream state and report how the
    /// stream ended.
    ///
    /// Whoever removes the entry reports the end: `close-stream` and the pump
    /// race, and the WIT contract promises exactly one terminal event per
    /// stream.
    async fn finish(self: &Arc<Self>, app: &str, id: &str, failure: Option<String>) {
        if self.entries.lock().remove(id).is_none() {
            return;
        }
        let _ = self.store.unregister_stream(app, id);
        let event = failure.map_or(StreamEvent::Closed, StreamEvent::Error);
        if let Err(e) = self.event(app, id, event).await {
            tracing::debug!(app = %app, stream = %id, error = %e, "terminal stream event failed");
        }
    }

    /// Pump one connected stream: forward chunks both ways and report the end.
    ///
    /// The queue closing (`None` from `recv`) is the guest's `close-stream`: the
    /// pump still flushes what was already queued and closes the wire cleanly,
    /// which is what makes a WebSocket close a Close frame instead of a dropped
    /// socket.
    async fn pump<C, K>(
        self: Arc<Self>,
        app: String,
        id: String,
        mut source: C,
        mut sink: K,
        mut rx: mpsc::Receiver<Vec<u8>>,
        limits: Limits,
    ) where
        C: ChunkSource + 'static,
        K: ChunkSink + 'static,
    {
        if let Err(e) = self.event(&app, &id, StreamEvent::Opened).await {
            tracing::debug!(stream = %id, error = %e, "opened event failed");
        }

        let max_frame = limits.max_frame.max(1);
        let mut failure: Option<String> = None;
        loop {
            tokio::select! {
                // A write the guest asked for (`send-stream`).
                command = rx.recv() => {
                    match command {
                        Some(bytes) if bytes.len() > max_frame => {
                            failure = Some(format!(
                                "io::send-stream: {} bytes exceeds the {max_frame} byte frame cap",
                                bytes.len()
                            ));
                            break;
                        }
                        Some(bytes) => {
                            if let Err(e) = sink.write(&bytes).await {
                                failure = Some(format!("write failed: {e:#}"));
                                break;
                            }
                        }
                        // The queue closed without a command: `close-stream`
                        // already reported the end.
                        None => break,
                    }
                }
                read = tokio::time::timeout(limits.idle, source.next()) => {
                    match read {
                        Err(_) => {
                            failure = Some(format!("idle for {}s", limits.idle.as_secs()));
                            break;
                        }
                        Ok(Ok(None)) => break,
                        Ok(Ok(Some(chunk))) => {
                            // A peer message larger than the cap (a WebSocket
                            // frame is not bounded by the socket's buffer) is
                            // delivered in `max_frame` slices: the guest
                            // reassembles with the carry-over buffer, and the
                            // frame is never silently truncated.
                            for slice in chunk.chunks(max_frame) {
                                let reply = match self.chunk(&app, &id, slice.to_vec()).await {
                                    Ok(reply) => reply,
                                    Err(e) => {
                                        failure = Some(format!("handle-stream failed: {e}"));
                                        break;
                                    }
                                };
                                if !reply.is_empty()
                                    && let Err(e) = sink.write(&reply).await
                                {
                                    failure = Some(format!("write failed: {e:#}"));
                                    break;
                                }
                            }
                            if failure.is_some() {
                                break;
                            }
                        }
                        Ok(Err(e)) => {
                            failure = Some(format!("read failed: {e:#}"));
                            break;
                        }
                    }
                }
            }
        }
        sink.close().await;
        self.finish(&app, &id, failure).await;
    }

    /// Register `id` with a queue, spawn its pump and record the abort handle.
    // The wiring of one stream: its halves, the identity it is announced under,
    // the policy it enforces and the listener it belongs to. Grouping them into
    // a struct would only move the same fields one call away.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_stream<C, K>(
        self: &Arc<Self>,
        app: &str,
        id: &str,
        info: StreamInfo,
        source: C,
        sink: K,
        limits: Limits,
        parent: Option<&str>,
    ) where
        C: ChunkSource + 'static,
        K: ChunkSink + 'static,
    {
        // Registered with its write queue before the guest is told, so
        // `stream-describe`, `send-stream` and a `close-stream` racing the pump's
        // first poll all see the stream.
        let (tx, rx) = mpsc::channel::<Vec<u8>>(WRITE_QUEUE);
        let (done_tx, done_rx) = oneshot::channel::<()>();
        self.announce(app, id, info);
        self.entries.lock().insert(
            id.to_string(),
            Entry {
                id: id.to_string(),
                app: app.to_string(),
                sender: Some(tx),
                abort: None,
                done: Some(done_rx),
                parent: parent.map(str::to_string),
            },
        );
        let core = Arc::clone(self);
        let app_owned = app.to_string();
        let pump_id = id.to_string();
        let task = tokio::spawn(async move {
            core.pump(app_owned, pump_id, source, sink, rx, limits)
                .await;
            // Whoever is waiting in `close-stream` learns the wire is closed.
            let _ = done_tx.send(());
        });
        if let Some(entry) = self.entries.lock().get_mut(id) {
            entry.abort = Some(task.abort_handle());
        }
    }

    /// Tear one live stream or listener down and report its end exactly once.
    ///
    /// The write queue is closed first (the guest can no longer write), then the
    /// pump is given [`CLOSE_GRACE`] to flush the queue and close the wire, and
    /// only after that is the deadline enforced by aborting the task. A listener
    /// was never announced to the guest, so it reports no event of its own.
    pub(super) async fn teardown(&self, entry: Entry) {
        let Entry {
            id,
            app,
            sender,
            abort,
            done,
            parent,
        } = entry;
        let announced = sender.is_some() || parent.is_some();
        // Dropping the queue's sender (the only one the pump waits on) ends
        // `recv` with `None` once the chunks already queued have been written.
        drop(sender);
        if let Some(done) = done {
            if tokio::time::timeout(CLOSE_GRACE, done).await.is_err() {
                if let Some(abort) = abort {
                    abort.abort();
                }
                tracing::debug!(
                    app = %app,
                    stream = %id,
                    "stream did not close within the grace period; aborted"
                );
            }
        } else if let Some(abort) = abort {
            // A listener holds no completion channel: aborting its accept loop
            // is the only shutdown it has.
            abort.abort();
        }
        if announced {
            let _ = self.store.unregister_stream(&app, &id);
            let _ = self.event(&app, &id, StreamEvent::Closed).await;
        }
    }
}
