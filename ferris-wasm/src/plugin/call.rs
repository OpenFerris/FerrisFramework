//! Entering and leaving a guest export: the instance lock, the epoch deadline
//! and the re-entry flag.

use crate::FerrisApp;
use crate::error::{Result as FResult, WasmError};
use crate::host_impl::RealHostState;

use super::instance::Plugin;

thread_local! {
    /// True while the current thread is executing a guest export. A `before-*`
    /// lifecycle hook raised from *inside* a guest call — a plugin calling
    /// `docstore.create`/`update` from `handle-stream` or `handle-request` —
    /// cannot re-enter the already-locked instance, so that hook is skipped
    /// instead of deadlocking on the store lock.
    pub(super) static IN_GUEST_CALL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the current thread is inside a guest export (on any plugin). Lets
/// the synchronous `before-*` dispatch detect re-entry.
#[must_use]
pub(crate) fn in_guest_call() -> bool {
    IN_GUEST_CALL.with(std::cell::Cell::get)
}

/// Restores [`IN_GUEST_CALL`] to its previous value on drop, so the flag is
/// correct even if the guest call unwinds.
pub(super) struct GuestCallGuard(pub(super) bool);

impl Drop for GuestCallGuard {
    fn drop(&mut self) {
        let previous = self.0;
        IN_GUEST_CALL.with(|flag| flag.set(previous));
    }
}

/// Epoch ticks budgeted for one **guest call** (≈10 s at the default 10 ms
/// tick). Every export (`invoke`, `doctype_event`) re-arms this before entry.
///
/// This must be well above a single tick, and above any legitimate host-side
/// import. The epoch clock keeps ticking while a host import runs, so a call
/// that crosses into a cold docstore query (first SQLite pool hit, DDL, catalog
/// materialization) is charged for that wall time; a 1 s budget trapped those
/// calls at the first guest checkpoint instead of only catching runaway loops.
/// Ten seconds matches the instantiation budget and still interrupts a true
/// infinite loop. See `SolmecBOX/docs/SEED-TRAP.md` for the observed 2–3 s host
/// paths this covers.
pub(super) const PER_CALL_EPOCH_BUDGET: u64 = 1_000;

impl Plugin {
    /// Run `f` against the plugin's store, with the epoch deadline armed so a
    /// runaway plugin is trapped instead of blocking the host.
    #[allow(clippy::significant_drop_tightening)] // the lock guard must outlive `f`, which borrows the store; the mutex is not reentrant
    pub(super) fn invoke<T>(
        &self,
        f: impl FnOnce(
            &mut wasmtime::Store<RealHostState>,
            &FerrisApp,
        ) -> std::result::Result<T, WasmError>,
    ) -> FResult<T> {
        let mut store = self.store.lock();
        let was = IN_GUEST_CALL.with(|flag| flag.replace(true));
        let _call_guard = GuestCallGuard(was);
        store.set_epoch_deadline(PER_CALL_EPOCH_BUDGET);
        f(&mut store, &self.instance)
    }
}
