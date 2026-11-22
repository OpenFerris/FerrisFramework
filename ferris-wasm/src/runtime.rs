use std::num::NonZeroUsize;
use std::time::Duration;

/// Tunables applied to the one shared `wasmtime::Engine`.
///
/// Every option is a deliberate trade-off for a shared, multi-plugin runtime:
/// safety ceiling first (epoch interrupts, stack + memory caps, backtraces
/// off), then raw speed within that ceiling (`CraneliftOptLevel::SpeedAndSize`).
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // one field per independent engine tunable
pub struct RuntimeConfig {
    /// Ceiling for a single plugin instance's linear memory. Guards against a
    /// plugin (or a malicious bytecode slice) exhausting the host.
    pub max_memory_bytes: usize,
    /// Maximum native stack a plugin may consume; keeps traps from taking down
    /// the host.
    pub max_wasm_stack_bytes: usize,
    /// Enable epoch interruption: a plugin that runs too long between host
    /// checkpoints is trapped instead of hanging the process.
    pub enable_epoch_interrupts: bool,
    /// How often the background clock bumps the shared engine's epoch.
    pub epoch_increment_ms: u64,
    /// Whether wasm backtraces are collected on traps. Off is safer and faster.
    pub collect_backtraces: bool,
    /// Compiler optimization target for the shared engine.
    pub opt_level: wasmtime::OptLevel,
    /// Canonicalize NaNs. Breaks leakage of host NaN bit patterns.
    pub nan_canonicalization: bool,
    /// Keep compiled artifacts in wasmtime's on-disk cache.
    ///
    /// Off by default: a library should not write to disk behind the caller's
    /// back. `ferris dev` turns it on, because there the same component is
    /// loaded over and over and recompiling it from cranelift every time was
    /// the bulk of a hot reload (~800 ms for a 3.5 MiB app).
    pub compile_cache: bool,
    /// Enable the WebAssembly threads proposal (atomics and shared memories),
    /// which is what lets a guest that starts its own threads instantiate.
    ///
    /// On by default because it only *permits* the proposal: a component that
    /// does not import shared memory compiles exactly as before, and the
    /// per-instance [`wasmtime::StoreLimits`] still bounds every memory
    /// (shared ones included), so a thread pool cannot escape the memory
    /// ceiling. The `threads` capability is granted separately, per app; this
    /// switch is the engine-side half of that decision, and
    /// `platform::features` reports `threads` only while it is on.
    pub wasm_threads: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: 64 * 1024 * 1024,
            max_wasm_stack_bytes: 1024 * 1024,
            enable_epoch_interrupts: true,
            epoch_increment_ms: 10,
            collect_backtraces: false,
            opt_level: wasmtime::OptLevel::SpeedAndSize,
            nan_canonicalization: false,
            compile_cache: false,
            wasm_threads: true,
        }
    }
}

impl RuntimeConfig {
    /// Apply these settings onto an off-the-shelf `wasmtime::Config`.
    fn apply(&self, config: &mut wasmtime::Config) -> crate::error::Result<()> {
        config
            .max_wasm_stack(self.max_wasm_stack_bytes)
            .cranelift_opt_level(self.opt_level)
            .cranelift_nan_canonicalization(self.nan_canonicalization);

        if self.enable_epoch_interrupts {
            config.epoch_interruption(true);
        }

        // Threads are opt-out, not opt-in: a guest that imports a shared memory
        // fails to instantiate without this, and the capability decides per app
        // whether the surface is granted.
        config.wasm_threads(self.wasm_threads);

        if self.collect_backtraces {
            config.wasm_backtrace_max_frames(NonZeroUsize::new(32));
        } else {
            config.wasm_backtrace_details(wasmtime::WasmBacktraceDetails::Disable);
        }

        if self.compile_cache {
            let cache_config = wasmtime::CacheConfig::new();
            let cache = wasmtime::Cache::new(cache_config)
                .map_err(|e| crate::error::WasmError::CompileCache(anyhow::anyhow!(e)))?;
            config.cache(Some(cache));
        }

        Ok(())
    }
}

/// The shared runtime. Create **one** per process and hand clones of it to
/// the plugin store and any other runtime consumer.
pub struct SharedRuntime {
    engine: wasmtime::Engine,
    config: RuntimeConfig,
}

impl std::fmt::Debug for SharedRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedRuntime")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl SharedRuntime {
    /// Build the shared engine from a [`RuntimeConfig`].
    ///
    /// # Errors
    ///
    /// Returns an error when the settings cannot be applied to the wasmtime
    /// engine (for example, an unsupported optimization level).
    pub fn new(config: RuntimeConfig) -> crate::error::Result<Self> {
        let mut wasm_config = wasmtime::Config::new();
        config.apply(&mut wasm_config)?;
        let engine = wasmtime::Engine::new(&wasm_config)?;
        Ok(Self { engine, config })
    }

    /// The single shared engine. Passed to every component instantiation.
    #[must_use]
    pub const fn engine(&self) -> &wasmtime::Engine {
        &self.engine
    }

    /// The active tunables.
    #[must_use]
    pub const fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// Start the epoch clock if interrupts are enabled.
    ///
    /// Returns `None` when interrupts are off. The task increments the shared
    /// engine's epoch every `epoch_increment_ms`, so any plugin that exceeds a
    /// deadline is interrupted at the next checkpoint. The handle is detached
    /// (runs for the process lifetime) unless the caller keeps it.
    #[must_use]
    pub fn spawn_epoch_clock(&self) -> Option<tokio::task::JoinHandle<()>> {
        if !self.config.enable_epoch_interrupts {
            return None;
        }
        let engine = self.engine.clone();
        let period = Duration::from_millis(self.config.epoch_increment_ms);
        Some(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(period);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                engine.increment_epoch();
            }
        }))
    }
}

impl Default for SharedRuntime {
    fn default() -> Self {
        Self::new(RuntimeConfig::default()).expect("shared wasmtime engine failed to start")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_accountable() {
        let config = RuntimeConfig::default();
        assert!(config.enable_epoch_interrupts);
        assert!(!config.collect_backtraces);
        assert!(config.max_memory_bytes > 0);
        assert!(config.max_wasm_stack_bytes > 0);
        assert!(config.epoch_increment_ms > 0);
        assert!(config.wasm_threads, "the threads proposal is on by default");
    }

    /// `wasm_threads` is the engine-side half of the `threads` capability: when
    /// it is on a guest with a shared memory instantiates, and when it is off
    /// the same module is refused at compile time (no silent coercion).
    #[test]
    fn the_threads_switch_decides_whether_shared_memory_compiles() {
        let shared = r#"(module (memory 1 1 shared))"#;

        let on = SharedRuntime::new(RuntimeConfig {
            enable_epoch_interrupts: false,
            wasm_threads: true,
            ..Default::default()
        })
        .unwrap();
        let bytes = wat::parse_str(shared).expect("sample wat");
        assert!(
            wasmtime::Module::new(on.engine(), bytes).is_ok(),
            "a shared memory must compile when threads are enabled"
        );

        let off = SharedRuntime::new(RuntimeConfig {
            enable_epoch_interrupts: false,
            wasm_threads: false,
            ..Default::default()
        })
        .unwrap();
        let bytes = wat::parse_str(shared).expect("sample wat");
        assert!(
            wasmtime::Module::new(off.engine(), bytes).is_err(),
            "a shared memory must be refused when threads are disabled"
        );
    }

    #[test]
    fn runtime_builds_and_compiles_wasm() {
        // Epoch interrupts are left off: a store traps immediately with an
        // elapsed deadline unless `set_epoch_deadline` is called, which would
        // muddy this smoke test.
        let runtime = SharedRuntime::new(RuntimeConfig {
            enable_epoch_interrupts: false,
            ..Default::default()
        })
        .unwrap();
        let module_wat = r#"
            (module
              (func (export "add") (param i32 i32) (result i32)
                local.get 0
                local.get 1
                i32.add))
        "#;
        let bytes = wat::parse_str(module_wat).expect("sample wat");
        let module = wasmtime::Module::new(runtime.engine(), bytes).expect("compile");
        let mut store = wasmtime::Store::new(runtime.engine(), ());
        let linker = wasmtime::Linker::new(runtime.engine());
        let instance = linker
            .instantiate(&mut store, &module)
            .expect("instantiate");
        let add = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "add")
            .unwrap();
        assert_eq!(add.call(&mut store, (2, 40)).unwrap(), 42);
    }

    #[test]
    fn epoch_clock_is_spawned_only_when_enabled() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();

        let on = SharedRuntime::new(RuntimeConfig::default()).unwrap();

        let off = SharedRuntime::new(RuntimeConfig {
            enable_epoch_interrupts: false,
            ..Default::default()
        })
        .unwrap();
        rt.block_on(async {
            assert!(on.spawn_epoch_clock().is_some());
        });

        rt.block_on(async {
            assert!(off.spawn_epoch_clock().is_none());
        });
    }
}
