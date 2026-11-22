//! Trap recovery: replace a poisoned instance with a fresh one.

use tracing::{error, warn};

use super::registry::PluginStore;

impl PluginStore {
    /// Surface a guest call's error, first replacing a trapped instance.
    ///
    /// A trap leaves the `wasmtime::Store` poisoned: wasmtime refuses to re-enter
    /// that instance, so every later call fails with `cannot enter component
    /// instance` and the store would stay wedged for the rest of the process.
    /// When the component bytes are known ([`Self::remember`]), a trap here
    /// recompiles and publishes a fresh instance before the error reaches the
    /// caller; the failing call itself still returns its error. `plugin` is the
    /// app the call went to, so only that app is rebuilt (SRV17).
    pub(super) fn guard_trap<T>(
        &self,
        res: std::result::Result<T, crate::WasmError>,
        plugin: &str,
    ) -> std::result::Result<T, crate::WasmError> {
        if matches!(&res, Err(crate::WasmError::Trap { .. })) {
            self.recover_after_trap(plugin);
        }
        res
    }

    /// Rebuild the app registered under `name` and publish it, after a trap
    /// poisoned its instance.
    ///
    /// Returns `false` — logging why — when there is no remembered source (the
    /// plugin arrived through a path that bypassed the store) or the rebuild
    /// fails; the caller still surfaces the trap either way. Schema and hooks
    /// are properties of the app, not of one instance, so nothing is re-wired:
    /// the fresh instance simply replaces the poisoned one.
    pub(super) fn recover_after_trap(&self, name: &str) -> bool {
        let remembered = self
            .sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned();
        let Some(bytes) = remembered else {
            warn!(plugin = %name, "wasm trap but no remembered component source; cannot recover");
            return false;
        };
        match self.compile(&bytes, name) {
            Ok(plugin) => {
                warn!(plugin = %name, "recovering from wasm trap: publishing a fresh instance");
                self.swap(plugin);
                true
            }
            Err(e) => {
                error!(plugin = %name, error = %e, "failed to rebuild plugin after trap");
                false
            }
        }
    }
}
