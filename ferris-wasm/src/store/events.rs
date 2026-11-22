//! Lifecycle-event dispatch: the synchronous before-points and the deferred
//! queue drained by a dedicated thread.

use std::sync::{Arc, mpsc};

use anyhow::Result;
use tracing::{error, warn};

use ferris_auth::User;

use super::registry::PluginStore;
use crate::Plugin;

impl PluginStore {
    /// Forward a `DocType` lifecycle event to the app that owns `doctype`
    /// (no-op when no loaded app declares it). If the owning wasm instance is
    /// busy -- either re-entering from inside the app's own active call, or
    /// held by another thread's request -- the event is queued and delivered by
    /// the background drainer as soon as the instance is idle, so host-driven
    /// events are never lost. This (fire-and-forget) path is for the **after**
    /// points (`created`, `after-save`, …), which only observe a write that
    /// already happened.
    ///
    /// Only the owner receives the document: addressing the most recently
    /// loaded app sent one app another app's JSON, skipped the owner's own
    /// validation and let the last-loaded app veto writes it does not own
    /// (P0-6/WASM-003/SEC-05). An unowned `DocType` is dropped, not broadcast.
    ///
    /// `user` is the ambient identity of the operation that raised the event;
    /// the plugin's `session` imports see it (fail-closed when `None`).
    ///
    /// # Errors
    ///
    /// Returns an error when the owning plugin's `doctype_event` export fails.
    pub fn on_doctype_event(
        &self,
        event: &str,
        doctype: &str,
        name: &str,
        data: &str,
        user: Option<User>,
    ) -> Result<()> {
        let Some(plugin) = self.owner_plugin(doctype) else {
            self.note_unowned_doctype(doctype);
            return Ok(());
        };
        let reentrant = DISPATCHING.with(|d| {
            if d.get() {
                true
            } else {
                d.set(true);
                false
            }
        });
        if reentrant {
            self.enqueue(event, doctype, name, data, user);
            return Ok(());
        }
        let _guard = DispatchingGuard;
        let plugin_name = plugin.name().to_string();
        let res = plugin.try_on_doctype_event(event, doctype, name, data, user.clone());
        let dispatched = match self.guard_trap(res, &plugin_name) {
            Ok(v) => v,
            Err(e) => {
                error!(event, doctype, name, error=%e, "try_on_doctype_event failed");
                return Err(e.into());
            }
        };
        if dispatched != Some(()) {
            self.enqueue(event, doctype, name, data, user);
        }
        Ok(())
    }

    /// Forward a **before** point (`before-insert`, `before-save`) to the app
    /// that owns `doctype` and let its verdict decide the write: the guest's
    /// `error` propagates so the engine aborts the row. Unlike
    /// [`Self::on_doctype_event`] this never queues — a deferred verdict would
    /// arrive after the write it was meant to gate — so it blocks on the
    /// instance when another thread holds it.
    ///
    /// A `DocType` no loaded app owns has no plugin gate: the point is not
    /// delivered anywhere (there is nothing to ask) and the write proceeds.
    /// What it must **not** do is ask the most recently loaded app, which would
    /// let one app reject another app's writes (P0-6/SEC-05).
    ///
    /// When the event is raised from *inside* the plugin's own call (`docstore`
    /// write) the single instance cannot be re-entered, so the point cannot be
    /// run. The write is **refused** with a validation error instead of being
    /// allowed unvalidated: a plugin that intends to skip its own events uses
    /// the explicit `docstore::write-no-events`, which the hook checks before
    /// this method is reached.
    ///
    /// # Errors
    ///
    /// Returns the plugin's rejection when its `doctype_event` export fails, and
    /// a validation error when a `before-*` point cannot run because the guest
    /// instance is already executing.
    pub fn on_doctype_event_pre(
        &self,
        event: &str,
        doctype: &str,
        name: &str,
        data: &str,
        user: Option<User>,
    ) -> std::result::Result<(), crate::WasmError> {
        // Resolve before touching the instance: `in_guest_call` is checked
        // first, below, and this lookup is host-side only (no guest call), so
        // neither branch can deadlock on a locked instance.
        let Some(plugin) = self.owner_plugin(doctype) else {
            self.note_unowned_doctype(doctype);
            return Ok(());
        };
        // A `before-*` raised from inside a guest call — the plugin's own
        // `docstore.create`/`update` during `handle-stream`/`handle-request` —
        // cannot re-enter the locked instance. Refuse instead of deadlocking,
        // and instead of letting the write through unvalidated.
        if crate::plugin::in_guest_call() {
            return Err(Self::before_unavailable(
                &plugin,
                event,
                doctype,
                name,
                "inside the plugin's own guest call",
            ));
        }
        let reentrant = DISPATCHING.with(|d| {
            if d.get() {
                true
            } else {
                d.set(true);
                false
            }
        });
        if reentrant {
            return Err(Self::before_unavailable(
                &plugin,
                event,
                doctype,
                name,
                "while another doctype event is already dispatching",
            ));
        }
        let _guard = DispatchingGuard;
        let plugin_name = plugin.name().to_string();
        let res = plugin.on_doctype_event(event, doctype, name, data, user);
        self.guard_trap(res, &plugin_name)
    }

    /// The typed refusal for a `before-*` point that cannot be dispatched.
    ///
    /// `ValidationFailed` (422) because the guest asked to write without the
    /// validation it declared; the message names the explicit opt-out so the
    /// fix is mechanical.
    fn before_unavailable(
        plugin: &Plugin,
        event: &str,
        doctype: &str,
        name: &str,
        reason: &str,
    ) -> crate::WasmError {
        crate::WasmError::PluginError {
            name: plugin.name().to_string(),
            call: "doctype-event",
            kind: crate::PluginErrorKind::ValidationFailed,
            message: format!(
                "`{event}` cannot run {reason} for {doctype}/{name}; \
                 use `docstore::write-no-events` to skip plugin events explicitly"
            ),
        }
    }

    /// Warn once that no loaded app owns `doctype`, so a busy install reports
    /// the misconfiguration instead of silently observing nothing.
    fn note_unowned_doctype(&self, doctype: &str) {
        if self.notice_once(format!("doctype:{doctype}")) {
            warn!(
                doctype,
                "doctype event has no owning app; dropped (fail-closed)"
            );
        }
    }

    /// Drop an event into the pending queue and wake the background drainer.
    fn enqueue(&self, event: &str, doctype: &str, name: &str, data: &str, user: Option<User>) {
        let queued = QueuedEvent {
            event: event.to_string(),
            doctype: doctype.to_string(),
            name: name.to_string(),
            data: data.to_string(),
            user,
            attempts: 0,
        };
        let pending;
        {
            let mut queue = self.pending.lock();
            if queue.len() >= MAX_PENDING {
                tracing::warn!(
                    event,
                    doctype,
                    name,
                    "doctype event dropped (pending queue full)"
                );
                return;
            }
            pending = queue.len() + 1;
            queue.push_back(queued);
        }
        tracing::info!(
            event,
            doctype,
            name,
            pending,
            "doctype event queued (instance busy)"
        );
        if let Some(tx) = self.wake.get() {
            let _ = tx.send(());
        }
    }

    /// Start the dedicated event drainer once. It owns its own tokio runtime
    /// because queued dispatches block on the instance lock while the guest
    /// host imports resolve via `Handle::current().block_on`. Safe to call
    /// from any context (dev startup, hot reload) -- building a runtime
    /// requires an empty reactor, and both of those are outside one.
    pub(super) fn start_drainer(self: &Arc<Self>) {
        if self.wake.get().is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel::<()>();
        let _ = self.wake.set(tx);
        let store = self.clone();
        std::thread::Builder::new()
            .name("ferris-plugin-events".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        tracing::error!(error = %e, "failed to start plugin event drainer runtime");
                        return;
                    }
                };
                rt.block_on(drain_loop(store, rx));
            })
            .ok();
    }

    /// Deliver one queued event to the app that owns its `DocType`. Runs on the
    /// dedicated drainer thread and blocks on the instance lock; the
    /// re-entrancy guard re-queues anything this dispatch itself triggers. An
    /// event whose owner disappeared (or never existed) is dropped — the queue
    /// must not fall back to the most recently loaded app.
    fn dispatch_queued(&self, event: QueuedEvent) {
        let Some(plugin) = self.owner_plugin(&event.doctype) else {
            self.note_unowned_doctype(&event.doctype);
            return;
        };
        let reentrant = DISPATCHING.with(|d| {
            if d.get() {
                true
            } else {
                d.set(true);
                false
            }
        });
        if reentrant {
            self.pending.lock().push_back(event);
            return;
        }
        let _guard = DispatchingGuard;
        let plugin_name = plugin.name().to_string();
        let res = plugin.on_doctype_event(
            &event.event,
            &event.doctype,
            &event.name,
            &event.data,
            event.user.clone(),
        );
        match res {
            Err(crate::WasmError::Trap { .. }) => {
                if event.attempts >= MAX_TRAP_RETRIES || !self.recover_after_trap(&plugin_name) {
                    tracing::warn!(
                        event = %event.event,
                        doctype = %event.doctype,
                        name = %event.name,
                        "queued doctype event failed after trap"
                    );
                    return;
                }
                let mut retried = event;
                retried.attempts += 1;
                tracing::warn!(
                    event = %retried.event,
                    doctype = %retried.doctype,
                    name = %retried.name,
                    attempts = retried.attempts,
                    "queued doctype event trapped, retrying once after recovery"
                );
                self.pending.lock().push_front(retried);
                let _ = self.wake.get().map(|tx| tx.send(()));
            }
            Err(e) => {
                tracing::warn!(
                    event = %event.event,
                    doctype = %event.doctype,
                    name = %event.name,
                    error = %e,
                    "queued doctype event failed"
                );
            }
            Ok(()) => {}
        }
    }
}

/// One `doctype_event` awaiting delivery to an idle instance.
pub(super) struct QueuedEvent {
    event: String,
    doctype: String,
    name: String,
    data: String,
    user: Option<User>,
    attempts: u8,
}

/// Safety valve so a runaway guest (self-triggering events in a loop) cannot
/// grow the queue unboundedly; newest events are dropped once full.
const MAX_PENDING: usize = 1024;

/// How many times the drainer re-delivers one event that trapped, after
/// [`PluginStore::recover_after_trap`] swapped in a fresh instance. A guest
/// that traps deterministically on the same event must not spin the drainer
/// forever, so the event is dropped past this budget.
const MAX_TRAP_RETRIES: u8 = 2;

/// Background loop delivering queued events. Blocks on the wake channel, then
/// drains everything queued against whatever plugin is currently loaded.
/// Self-triggered events fired from inside a dispatch are re-queued by the
/// re-entrancy guard instead of deadlocking the single instance.
///
/// Stays `async` (though it never awaits) so the caller can drive it with
/// `block_on`: the `block_in_place` calls inside only work from inside a
/// multithreaded tokio runtime context.
#[allow(clippy::unused_async)]
async fn drain_loop(store: Arc<PluginStore>, rx: mpsc::Receiver<()>) {
    loop {
        let rx = &rx;
        let wake = tokio::task::block_in_place(move || rx.recv());
        if wake.is_err() {
            return;
        }
        while let Some(event) = { store.pending.lock().pop_front() } {
            let store = store.clone();
            tokio::task::block_in_place(move || store.dispatch_queued(event));
        }
    }
}

/// Re-entrancy guard: reset on drop even if the wasm call unwinds.
struct DispatchingGuard;

impl Drop for DispatchingGuard {
    fn drop(&mut self) {
        DISPATCHING.with(|d| d.set(false));
    }
}

thread_local! {
    pub(super) static DISPATCHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::{PluginStore, QueuedEvent};
    use crate::runtime::{RuntimeConfig, SharedRuntime};

    async fn store() -> PluginStore {
        let runtime = Arc::new(
            SharedRuntime::new(RuntimeConfig {
                enable_epoch_interrupts: false,
                ..Default::default()
            })
            .expect("SharedRuntime"),
        );
        PluginStore::new(runtime, ferris_db::memory_db().await.expect("memory db"))
    }

    /// A built fixture, or `None` on a fresh clone (the suite skips loudly).
    fn fixture(dir: &str, file: &str) -> Option<PathBuf> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "tests/fixtures/{dir}/target/wasm32-wasip2/debug/{file}.wasm"
        ));
        if path.exists() {
            Some(path)
        } else {
            eprintln!(
                "SKIPPED: cargo build --target wasm32-wasip2 \
                 --manifest-path ferris-wasm/tests/fixtures/{dir}/Cargo.toml"
            );
            None
        }
    }

    /// Load `probe-ok` as `a` and `probe-trap` as `b`, leaving `b` as the most
    /// recently swapped app — the state that used to receive everything.
    async fn two_apps() -> Option<PluginStore> {
        let ok = fixture("probe-ok", "probe_ok")?;
        let trap = fixture("probe-trap", "probe_trap")?;
        let store = store().await;
        store.swap_load_with_name(&ok, "a").expect("load a");
        store.swap_load_with_name(&trap, "b").expect("load b");
        assert_eq!(store.name().as_deref(), Some("b"));
        Some(store)
    }

    fn declare(store: &PluginStore, app: &str, doctype: &str) {
        store
            .synced
            .lock()
            .expect("synced")
            .insert((app.to_string(), doctype.to_string()), "fp".to_string());
    }

    /// P0-6/SEC-05: the event goes to the owner even though another app was
    /// swapped in last, and nothing is deferred for the loaded app.
    #[tokio::test]
    async fn a_doctype_event_addresses_the_owner_not_the_last_loaded_app() {
        let Some(store) = two_apps().await else {
            return;
        };
        declare(&store, "a", "Owned");

        store
            .on_doctype_event("created", "Owned", "1", "{}", None)
            .expect("the owner accepts the event");
        store
            .on_doctype_event_pre("before-save", "Owned", "1", "{}", None)
            .expect("the owner gates the write");
        assert!(
            store.pending.lock().is_empty(),
            "a delivered event must not be queued for another app"
        );
    }

    /// An unowned `DocType` reaches nobody: not the last-loaded app, not the
    /// queue. Fail-closed instead of broadcasting the document.
    #[tokio::test]
    async fn an_unowned_doctype_is_delivered_to_nobody() {
        let Some(store) = two_apps().await else {
            return;
        };
        store
            .on_doctype_event("created", "Nobody", "1", "{}", None)
            .expect("an unowned doctype is a no-op, not an error");
        store
            .on_doctype_event_pre("before-save", "Nobody", "1", "{}", None)
            .expect("no owner means no plugin gate");
        assert!(store.pending.lock().is_empty());
    }

    /// The drainer applies the same rule: an owner that disappeared drops the
    /// queued event instead of handing it to the loaded app.
    #[tokio::test]
    async fn a_queued_event_whose_owner_is_gone_is_dropped() {
        let Some(store) = two_apps().await else {
            return;
        };
        store.dispatch_queued(QueuedEvent {
            event: "created".to_string(),
            doctype: "Nobody".to_string(),
            name: "1".to_string(),
            data: "{}".to_string(),
            user: None,
            attempts: 0,
        });
        assert!(store.pending.lock().is_empty());
    }

    /// The owner still receives queued events after recovery from a trap.
    #[tokio::test]
    async fn a_queued_event_reaches_the_owner() {
        let Some(store) = two_apps().await else {
            return;
        };
        declare(&store, "a", "Owned");
        store.dispatch_queued(QueuedEvent {
            event: "created".to_string(),
            doctype: "Owned".to_string(),
            name: "1".to_string(),
            data: "{}".to_string(),
            user: None,
            attempts: 0,
        });
        assert!(store.pending.lock().is_empty());
    }
}
