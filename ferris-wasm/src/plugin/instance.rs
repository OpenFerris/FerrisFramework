//! The plugin instance: compile, instantiate and the per-instance state.

use crate::error::{Result as FResult, WasmError};
use crate::host_impl::RealHostState;
use crate::runtime::SharedRuntime;
use crate::{FerrisApp, StreamInfo};
use ferris_auth::User;
use parking_lot::Mutex;
use wasmtime::component::{self, Component, Linker};

use super::wasi::add_selective_wasi;

/// Epoch ticks budgeted for one component **instantiation** (≈10 s at the
/// default 10 ms tick, 1 s at the 1 ms tick tests use). The canonical-ABI
/// `_start` runs guest code, so a fresh store needs a real deadline — not the
/// wasmtime default of 0, which is "already elapsed" and traps immediately.
const INSTANTIATE_EPOCH_BUDGET: u64 = 1_000;

/// A loaded app plugin, pinned to the shared runtime's engine.
///
/// The plugin owns exactly one live instance (its `wasmtime::Store`). Calls go
/// through a single lock, so concurrent requests to the *same* plugin are
/// serialized while requests to *different* plugins run in parallel on the
/// shared engine — no per-plugin runtime, no process hops, minimal latency.
pub struct Plugin {
    pub(super) name: String,
    pub(super) instance: FerrisApp,
    pub(super) store: Mutex<wasmtime::Store<RealHostState>>,
    /// The network surface the app declared, kept host-side so the router can
    /// mount routes without calling back into the guest. `None` until
    /// `describe()` has run.
    pub(super) network: parking_lot::RwLock<Option<crate::descriptor::NetworkSpec>>,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugin")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Plugin {
    /// Compile + instantiate a `*.wasm` component for the `ferris-app` world.
    ///
    /// # Errors
    ///
    /// Returns an error when the component does not parse, compile, or
    /// instantiate against the framework world.
    pub fn from_bytes(
        runtime: &SharedRuntime,
        mut state: RealHostState,
        name: String,
        bytes: &[u8],
    ) -> FResult<Self> {
        let component = Component::from_binary(runtime.engine(), bytes).map_err(|e| {
            tracing::warn!(plugin = %name, error = %e, "component parse/compile failed");
            WasmError::Instantiate {
                name: name.clone(),
                source: e,
            }
        })?;

        // Per-instance resource ceiling so one plugin cannot eat the host.
        state.set_limits(
            wasmtime::StoreLimitsBuilder::new()
                .memory_size(runtime.config().max_memory_bytes)
                .instances(8)
                .memories(8)
                .build(),
        );
        // Namespace host-owned per-app settings (`config` keys) under this
        // plugin's identity before the component can call any import.
        state.set_plugin_name(name.clone());
        // The `threads` capability has no import to gate: the guest starts its
        // own threads, and the engine switch is what decides whether it can
        // instantiate at all. Record it so `platform::grants`/`features` answer
        // for this instance's engine; the per-instance memory ceiling above
        // still bounds shared memories, so a thread pool cannot escape it.
        state.set_threads_supported(runtime.config().wasm_threads);
        // The world is mounted per instance, after the grant is known and
        // before the component can reach an import; a world the host refuses
        // (symlinked root, unsafe name) fails the load instead of running the
        // app half-sandboxed.
        state
            .mount_world()
            .map_err(|source| WasmError::Instantiate {
                name: name.clone(),
                source,
            })?;
        let mounted_world = state.has_world();
        let mut store = wasmtime::Store::new(runtime.engine(), state);
        store.limiter(|host| host.limits_mut());

        // wasmtime 49 arms new stores with a deadline of 0 — "already elapsed" —
        // when epoch interruption is on, and component instantiation runs the
        // canonical-ABI `_start`, so without this the first checkpoint traps on
        // every load. Give the load a generous one-time budget; `invoke` re-arms
        // the per-call deadline before every guest call.
        store.set_epoch_deadline(INSTANTIATE_EPOCH_BUDGET);

        let mut linker = Linker::new(runtime.engine());
        FerrisApp::add_to_linker::<_, component::HasSelf<RealHostState>>(&mut linker, |data| data)
            .map_err(|e| WasmError::Instantiate {
                name: name.clone(),
                source: e,
            })?;
        // WASI preview2, selective: clocks/random/stdio/environment, the
        // filesystem only when this app's world was mounted, never sockets
        // (see `add_selective_wasi`).
        add_selective_wasi(&mut linker, mounted_world).map_err(|e| WasmError::Instantiate {
            name: name.clone(),
            source: e,
        })?;
        let instance = FerrisApp::instantiate(&mut store, &component, &linker).map_err(|e| {
            WasmError::Instantiate {
                name: name.clone(),
                source: e,
            }
        })?;

        Ok(Self {
            name,
            instance,
            store: Mutex::new(store),
            network: parking_lot::RwLock::new(None),
        })
    }

    /// Load a component from a file path.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or the component fails to
    /// compile/instantiate.
    pub fn from_file(
        runtime: &SharedRuntime,
        state: RealHostState,
        name: String,
        path: impl AsRef<std::path::Path>,
    ) -> FResult<Self> {
        let bytes = std::fs::read(path)?;
        Self::from_bytes(runtime, state, name, &bytes)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The network surface the app declared in `describe()`, or `None` while it
    /// has not described itself.
    ///
    /// The declaration is stored once per success and replaced on every
    /// reload, so the router reads the surface of the component that is
    /// actually running. Binding it — sockets, TLS, listeners — is the
    /// server's job; nothing here is executed.
    #[must_use]
    pub fn network(&self) -> Option<crate::descriptor::NetworkSpec> {
        self.network.read().clone()
    }

    /// Attach the ambient user of the operation to the host backing this
    /// plugin, so `session` imports and host docstore calls are authorized as
    /// the real caller. `None` resets to anonymous (fail-closed).
    pub fn set_current_user(&self, user: Option<User>) {
        self.store.lock().data().api().set_current_user(user);
    }

    /// The effective capability grant of this app, as `platform::grants`
    /// reports it.
    #[must_use]
    pub fn grants(&self) -> crate::host::GrantSet {
        self.store.lock().data().grant_set().clone()
    }

    /// Announce a live stream to the plugin's host so the `streams::info`
    /// import can answer for its `stream-id` (transport + peer).
    pub fn register_stream(&self, stream_id: &str, info: StreamInfo) {
        self.store
            .lock()
            .data()
            .api()
            .register_stream(stream_id, info);
    }

    /// Forget a closed stream: `streams::info` for its id now raises.
    pub fn unregister_stream(&self, stream_id: &str) {
        self.store.lock().data().api().unregister_stream(stream_id);
    }
}
