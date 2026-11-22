//! Store data attached to each plugin instance.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use wasmtime::component::ResourceTable;
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use super::api::HostApi;
use super::grants::{Capability, GrantSet};

// -- THE single bridge -------------------------------------------------------
//
// Every WIT imported interface (`ferris:framework/...`) generates its own
// `Host` trait; `FerrisApp::add_to_linker` requires the store data to
// implement all of them. The bridges used to be implemented twice (once per
// host state type) with identical bodies; now `HostState` is the one host the
// linker talks to and every import is forwarded here into [`HostApi`].
// Implementations that want a different backing (test doubles, `NullHost`)
// implement [`HostApi`], not the generated traits.

/// Where a plugin's world is mounted inside the guest. One stable path, so a
/// plugin can hardcode it and a tool (`wasm-tools`) can document it.
///
/// Paths are absolute (`/world/data/settings.json`): WASI preview 2 has no
/// process-wide working directory, so a relative path is not a portable
/// contract even though wasmtime resolves one against the preopen.
pub const GUEST_WORLD: &str = "/world";

/// Make sure the world's fixed sub-directories exist before the guest can
/// write into them.
fn ensure_world_subdirs(world: &Path) -> wasmtime::Result<()> {
    for relative in ferris_core::plugins::WORLD_DIRS {
        let dir = world.join(relative);
        std::fs::create_dir_all(&dir)
            .map_err(|e| wasmtime::Error::msg(format!("cannot create {}: {e}", dir.display())))?;
    }
    Ok(())
}

/// Store data attached to each plugin instance.
///
/// The [`HostApi`] is shared across every plugin instance, so the framework
/// services (pools, registries) are configured once. `limits` is per-instance
/// and is the resource ceiling the store applies on every memory/instance
/// growth.
pub struct HostState {
    api: std::sync::Arc<dyn HostApi>,
    limits: wasmtime::StoreLimits,
    wasi_ctx: WasiCtx,
    table: ResourceTable,
    /// The plugin this host instance belongs to; namespaces `config` keys (and
    /// any other per-app setting the host serializes).
    pub(super) plugin_name: String,
    /// What this app may reach at all (its capabilities). Starts at the
    /// internal set and is replaced by the intersection of the app's
    /// description once `describe()` has run.
    pub(super) grants: GrantSet,
    /// The pubsub channels the app declared, `None` while it has not described
    /// itself. `Some(set)` is enforcement: `io::publish` refuses a channel the
    /// declaration does not name.
    pub(super) subscriptions: Option<BTreeSet<String>>,
    /// Config keys the app declared as `secret`. The guest can never read one
    /// through `config`: secret material is operator-owned and only the host
    /// resolves it (TLS material), so the key stays out of the guest's reach
    /// whatever the app declares.
    pub(super) secret_config_keys: BTreeSet<String>,
    /// Directory that holds every app world (`plugins/`). `None` for a store
    /// built by a tool or a test that hosts no apps.
    worlds_root: Option<PathBuf>,
    /// The world mounted for this instance, once `mount_world` ran.
    world: Option<PathBuf>,
    /// `DocType` documents the app declared through `schema::declare-doctype` during
    /// `describe()`. Consumed by the plugin store after the call returns, so a
    /// declaration that fails validation never reaches the registry.
    pub(super) declared_doctypes: Vec<String>,
    /// Whether the shared engine has the WebAssembly threads proposal on, so a
    /// declared `threads` capability can be honoured. Set from
    /// [`RuntimeConfig`](crate::RuntimeConfig) at instantiation; `false` for a
    /// store that never saw an engine (a tool, a unit test), which keeps the
    /// grant out of `platform::grants` and `threads` out of
    /// `platform::features`.
    threads_supported: bool,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.table,
        }
    }
}

impl HostState {
    /// A host backed by an arbitrary [`HostApi`] implementation.
    pub fn new(api: std::sync::Arc<dyn HostApi>) -> Self {
        let mut builder = WasiCtxBuilder::new();
        // Stdout/stderr: inherit so guest logs reach the host console —
        // wasmtime documents concurrent printing as safe across instances.
        // stdin stays at the builder default (closed): plugins have no
        // legitimate stdin, and one inherited stdin shared by every pooled
        // instance is the exact "surprising results" scenario
        // `WasiCtxBuilder::inherit_stdin` warns about. Env/args also stay at
        // their defaults (empty): `inherit_env` would hand every plugin the
        // host's secrets.
        builder.inherit_stdout();
        builder.inherit_stderr();
        let wasi_ctx = builder.build();
        Self {
            api,
            limits: wasmtime::StoreLimitsBuilder::new().build(),
            wasi_ctx,
            table: ResourceTable::new(),
            plugin_name: String::new(),
            grants: GrantSet::primordial(),
            subscriptions: None,
            secret_config_keys: BTreeSet::new(),
            declared_doctypes: Vec::new(),
            worlds_root: None,
            world: None,
            threads_supported: false,
        }
    }

    /// The shared framework services behind this host.
    #[must_use]
    pub fn api(&self) -> &std::sync::Arc<dyn HostApi> {
        &self.api
    }

    /// Take the `DocType` documents declared through `schema::declare-doctype` since the
    /// last call. The vector is emptied so a second `describe()` cannot replay
    /// the same declarations.
    pub fn take_declared_doctypes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.declared_doctypes)
    }

    /// Per-instance resource ceiling the store applies.
    #[must_use]
    pub const fn limits(&self) -> &wasmtime::StoreLimits {
        &self.limits
    }

    /// Mutable access to the per-instance resource ceiling.
    ///
    /// Passed to `wasmtime::Store::limiter` so instance/memory growth is capped
    /// per plugin; the bridge field itself stays private so the ceiling can only
    /// be tuned through the same `set_limits`/`limits_mut` pair everywhere.
    pub const fn limits_mut(&mut self) -> &mut wasmtime::StoreLimits {
        &mut self.limits
    }

    /// Replace the per-instance resource ceiling (called at instantiation).
    pub const fn set_limits(&mut self, limits: wasmtime::StoreLimits) {
        self.limits = limits;
    }

    /// Bind this host to one plugin's identity (called at instantiation), so
    /// `config` keys are namespaced per app and its world can be resolved.
    pub fn set_plugin_name(&mut self, name: impl Into<String>) {
        self.plugin_name = name.into();
    }

    /// The directory that holds every app world (`plugins/`), set by the store
    /// that instantiated this host.
    pub fn set_worlds_root(&mut self, root: Option<PathBuf>) {
        self.worlds_root = root;
    }

    /// Record whether the engine this instance runs on has the threads
    /// proposal enabled, so a declared `threads` capability can be honoured.
    ///
    /// Called once at instantiation from
    /// [`RuntimeConfig::wasm_threads`](crate::RuntimeConfig::wasm_threads).
    /// There is no import to gate: the guest starts its own threads, and this
    /// flag is what `platform::grants`/`platform::features` report.
    pub const fn set_threads_supported(&mut self, supported: bool) {
        self.threads_supported = supported;
    }

    /// Whether the engine supports guest threads.
    #[must_use]
    pub const fn threads_supported(&self) -> bool {
        self.threads_supported
    }

    /// Mount the app's own world at [`GUEST_WORLD`], read-write.
    ///
    /// Called once per instantiation, after [`Self::set_plugin_name`] (the grant
    /// and the name are what decide it). The mount is the *entire* filesystem
    /// the guest can see: wasmtime resolves every path through the preopened
    /// directory, so `..`, absolute host paths and a symlink pointing out of the
    /// world are refused by the runtime, not by a check this function could be
    /// tricked into skipping. A symlinked **world root** is rejected here
    /// instead, because `preopened_dir` would happily open its target.
    ///
    /// Without the `filesystem` capability nothing is mounted: the guest's
    /// `wasi:filesystem` imports consequently have no linker implementation and
    /// the component does not instantiate (the fail-closed behaviour the
    /// selective linker documents).
    ///
    /// # Errors
    ///
    /// The world name is not a safe path segment, the world root is a symlink,
    /// or the directory cannot be created or opened.
    pub fn mount_world(&mut self) -> wasmtime::Result<()> {
        if self.world.is_some() || !self.grants.contains(Capability::Filesystem) {
            return Ok(());
        }
        let Some(root) = self.worlds_root.clone() else {
            // No plugin directory in this deployment (a tool-built store, a
            // test). The grant cannot conjure one; the guest simply has no
            // mount.
            return Ok(());
        };
        let world = ferris_core::plugins::world_root_at(&root, &self.plugin_name)
            .map_err(|e| wasmtime::Error::msg(format!("invalid app name for its world: {e}")))?;
        match std::fs::symlink_metadata(&world) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(wasmtime::Error::msg(format!(
                    "refusing to mount a symlinked world root: {}",
                    world.display()
                )));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                ferris_core::plugins::ensure_world_at(&root, &self.plugin_name).map_err(|e| {
                    wasmtime::Error::msg(format!("cannot create the app world: {e}"))
                })?;
            }
            Err(e) => {
                return Err(wasmtime::Error::msg(format!(
                    "cannot inspect the app world {}: {e}",
                    world.display()
                )));
            }
        }
        ensure_world_subdirs(&world)?;

        let mut builder = WasiCtxBuilder::new();
        builder.inherit_stdout();
        builder.inherit_stderr();
        // The guest's filesystem call runs on the blocking thread that already
        // hosts the whole plugin call (`block_in_place`); without this flag
        // wasmtime-wasi tries to start a second runtime on that thread and
        // panics. There is no async work to yield to: a world read is a read.
        builder.allow_blocking_current_thread(true);
        builder.preopened_dir(&world, GUEST_WORLD, FsPerms::ReadWrite)?;
        self.wasi_ctx = builder.build();
        self.world = Some(world);
        Ok(())
    }

    /// Whether a world is mounted for this instance.
    #[must_use]
    pub const fn has_world(&self) -> bool {
        self.world.is_some()
    }

    /// The mounted world, for logs and diagnostics.
    #[must_use]
    pub fn world(&self) -> Option<&Path> {
        self.world.as_deref()
    }

    /// The plugin name this host instance serves.
    #[must_use]
    pub fn plugin_name(&self) -> &str {
        &self.plugin_name
    }

    /// The effective capability grant of this app.
    #[must_use]
    pub const fn grant_set(&self) -> &GrantSet {
        &self.grants
    }

    /// Replace the grant (descriptor installation, tests).
    ///
    /// `describe()` recomputes it from the component that is actually running;
    /// there is no operator input to preserve across a reload.
    pub fn set_grants(&mut self, grants: GrantSet) {
        self.grants = grants;
    }

    /// Record the pubsub channels the app declared, so `io::publish` can
    /// enforce the declaration. `None` means "not described yet" and leaves
    /// the pre-13 behaviour in place.
    pub fn set_subscriptions(&mut self, channels: Option<BTreeSet<String>>) {
        self.subscriptions = channels;
    }

    /// Whether `channel` may be published to: always while the app has not
    /// described itself, and only when declared afterwards.
    #[must_use]
    pub fn may_publish(&self, channel: &str) -> bool {
        self.subscriptions
            .as_ref()
            .is_none_or(|declared| declared.contains(channel))
    }

    /// Record the config keys the app declared as `secret`, so reading one can
    /// require the `secrets` capability.
    pub fn set_secret_config_keys(&mut self, keys: BTreeSet<String>) {
        self.secret_config_keys = keys;
    }

    /// Whether the app declared `key` as a `secret` config key.
    #[must_use]
    pub fn is_secret_config_key(&self, key: &str) -> bool {
        self.secret_config_keys.contains(key)
    }

    /// Refuse a call whose capability this app was not granted.
    ///
    /// This is the one gate every import goes through: the typed
    /// `permission-denied` variant names the capability, so a guest can branch
    /// on it and an operator can read the log.
    ///
    /// # Errors
    ///
    /// Returns the WIT `permission-denied` variant when `capability` is not in
    /// the effective grant.
    pub(crate) fn require(
        &self,
        capability: Capability,
    ) -> Result<(), crate::ferris::framework::types::Error> {
        if self.grants.contains(capability) {
            return Ok(());
        }
        Err(crate::ferris::framework::types::Error::PermissionDenied(
            format!("capability `{capability}` is not granted to this app"),
        ))
    }
}

impl std::fmt::Debug for HostState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostState").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::NullHost;

    fn host() -> HostState {
        HostState::new(std::sync::Arc::new(NullHost))
    }

    /// A specific capability the app has not declared is refused, and the
    /// message names it.
    #[test]
    fn an_undeclared_specific_capability_is_permission_denied() {
        let host = host();
        let error = host
            .require(Capability::HttpUnrestricted)
            .expect_err("a fresh host has not declared the unrestricted egress");
        match error {
            crate::ferris::framework::types::Error::PermissionDenied(message) => {
                assert!(message.contains("`http-unrestricted`"), "{message}");
            }
            other => panic!("expected permission-denied, got {other:?}"),
        }
    }

    /// The primordial set is always on: the world, routes, outbound HTTP and
    /// the framework services need no declaration.
    #[test]
    fn primordial_capabilities_are_always_granted() {
        let host = host();
        for capability in [
            Capability::Filesystem,
            Capability::Routes,
            Capability::Http,
            Capability::DocstoreRead,
            Capability::DocstoreWrite,
            Capability::Config,
            Capability::Schema,
            Capability::Scheduler,
            Capability::Notifications,
            Capability::Pubsub,
        ] {
            assert!(host.require(capability).is_ok(), "{capability}");
        }
    }

    /// A specific capability is granted the moment the app declares it.
    #[test]
    fn a_declared_specific_capability_is_granted() {
        let mut host = host();
        host.set_grants(GrantSet::for_declaration([Capability::TlsClient]));
        assert!(host.require(Capability::TlsClient).is_ok());
        assert!(host.require(Capability::TlsServer).is_err());
    }

    /// Before the app describes itself there is no declaration to enforce, and
    /// once it does, publishing outside its channels is refused.
    #[test]
    fn publish_is_scoped_to_the_declared_channels() {
        let mut host = host();
        assert!(host.may_publish("anything"), "nothing declared yet");
        host.set_subscriptions(Some(std::iter::once("sensor:east".to_string()).collect()));
        assert!(host.may_publish("sensor:east"));
        assert!(!host.may_publish("sensor:west"));
    }
}
