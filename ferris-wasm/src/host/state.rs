//! Store data attached to each plugin instance.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use wasmtime::component::ResourceTable;
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use super::api::HostApi;
use super::grants::{ApprovedGrants, Capability, GrantSet};

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
    /// description once `describe()` has run. This is the manifest request;
    /// [`Self::approved`] is the operator's decision.
    pub(super) grants: GrantSet,
    /// The operator-approved grants (`grants.toml`), loaded at instantiation.
    /// Missing means nothing is approved: every non-primordial capability is
    /// denied until `ferris app install` writes the file.
    pub(super) approved: ApprovedGrants,
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
            approved: ApprovedGrants::default(),
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
    /// the operator approval is separate and survives a reload.
    pub fn set_grants(&mut self, grants: GrantSet) {
        self.grants = grants;
    }

    /// Load the operator's approval for this app from its directory
    /// (`<worlds root>/<plugin name>`), preferring `grants.toml` and falling
    /// back to `world/grants.toml`.
    ///
    /// Called once at instantiation, after [`Self::set_plugin_name`]. Without a
    /// worlds root (a tool or a test that hosts no apps) no file can be found,
    /// so nothing is approved. A file that exists but cannot be read or parsed
    /// fails the instance load: a broken approval must not degrade into
    /// "nothing approved" silently, because the operator would not know why
    /// every specific capability stopped working.
    ///
    /// # Errors
    ///
    /// The app name is not a safe path segment, or the grants file cannot be
    /// read or parsed.
    pub fn load_approved_grants(&mut self) -> wasmtime::Result<()> {
        let Some(root) = self.worlds_root.as_deref() else {
            self.approved = ApprovedGrants::default();
            return Ok(());
        };
        let app_dir =
            ferris_core::plugins::world_root_at(root, &self.plugin_name).map_err(|e| {
                wasmtime::Error::msg(format!(
                    "cannot resolve the app directory for the approved grants of `{}`: {e}",
                    self.plugin_name
                ))
            })?;
        self.approved = ApprovedGrants::load(&app_dir).map_err(|error| {
            wasmtime::Error::msg(format!(
                "cannot read the approved grants for app `{}` in {}: {error}",
                self.plugin_name,
                app_dir.display()
            ))
        })?;
        Ok(())
    }

    /// Install an approval explicitly (a test, a tool, or a host that loaded it
    /// from another layout). `None` is the fail-closed "no file" state.
    pub fn set_approved_grants(&mut self, grants: Option<crate::trust::GrantSet>) {
        self.approved = grants.map_or_else(ApprovedGrants::default, ApprovedGrants::from_set);
    }

    /// The operator-approved grants loaded for this app, if any.
    #[must_use]
    pub fn approved_grants(&self) -> Option<&crate::trust::GrantSet> {
        self.approved.grant_set()
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
    /// Two gates apply:
    ///
    /// * The **manifest** request must contain the capability — the pre-trust
    ///   rule, unchanged.
    /// * For every **non-primordial** capability, the operator's
    ///   [`ApprovedGrants`] must also name it. A manifest request is not a
    ///   grant; a missing `grants.toml` denies with the app and the files the
    ///   host looked for.
    ///
    /// **Primordial** capabilities (`routes`, `http`, `docstore-read`,
    /// `docstore-write`, `filesystem`, `config`, `schema`, `scheduler`,
    /// `notifications`, `pubsub`) keep the old rule — the manifest alone —
    /// because they are the services an app needs to exist and the host's own
    /// policy already bounds them. Requiring a per-app approval for them would
    /// duplicate that policy, not add one.
    ///
    /// # Errors
    ///
    /// Returns the WIT `permission-denied` variant when the manifest does not
    /// grant `capability`, or when it is non-primordial and not approved.
    pub(crate) fn require(
        &self,
        capability: Capability,
    ) -> Result<(), crate::ferris::framework::types::Error> {
        if !self.grants.contains(capability) {
            return Err(crate::ferris::framework::types::Error::PermissionDenied(
                format!("capability `{capability}` is not granted to this app"),
            ));
        }
        if capability.primordial() || self.approved.allows_capability(capability) {
            return Ok(());
        }
        Err(crate::ferris::framework::types::Error::PermissionDenied(
            self.approval_denial(capability),
        ))
    }

    /// The message for a non-primordial capability the operator did not
    /// approve, naming the app and either the file that withheld it or every
    /// file the host looked for.
    fn approval_denial(&self, capability: Capability) -> String {
        if self.approved.grant_set().is_none() {
            return format!(
                "capability `{capability}` is denied for app `{}`: no approved {} (looked at {})",
                self.plugin_name,
                crate::trust::GRANTS_FILE,
                self.approved.candidates_label()
            );
        }
        match self.approved.loaded_from() {
            Some(path) => format!(
                "capability `{capability}` is not approved for app `{}` in {}",
                self.plugin_name,
                path.display()
            ),
            None => format!(
                "capability `{capability}` is not approved for app `{}` by the operator",
                self.plugin_name
            ),
        }
    }

    /// Refuse an outbound HTTP target the app did not declare or the operator
    /// did not approve.
    ///
    /// The request's `http-policy.allowed-hosts` list (`declared`) is the
    /// manifest's narrowing and is checked first; an empty list means "no
    /// narrowing", matching the transport. The operator approval must then name
    /// the host in `grants.toml` — exact match, exactly like
    /// [`crate::trust::GrantSet::allows_http_host`].
    ///
    /// `http-unrestricted` keeps its documented meaning of skipping the host
    /// allow-list, so the bridge does not call this for an unrestricted call;
    /// the capability itself is still operator-gated by [`Self::require`].
    ///
    /// # Errors
    ///
    /// `permission-denied` when the host is outside the declaration, or when
    /// the operator's approval does not name it (including a missing file).
    pub fn require_http_host(
        &self,
        host: &str,
        declared: &[String],
    ) -> Result<(), crate::ferris::framework::types::Error> {
        if !declared.is_empty() && !declared_host(declared, host) {
            return Err(crate::ferris::framework::types::Error::PermissionDenied(
                format!("host `{host}` is outside this app's declared `allowed-hosts`"),
            ));
        }
        if self.approved.allows_http_host(host) {
            return Ok(());
        }
        let detail = if self.approved.grant_set().is_none() {
            format!(
                "app `{}` has no approved {} (looked at {})",
                self.plugin_name,
                crate::trust::GRANTS_FILE,
                self.approved.candidates_label()
            )
        } else {
            match self.approved.loaded_from() {
                Some(path) => format!("the operator did not approve it in {}", path.display()),
                None => "the operator did not approve it".to_string(),
            }
        };
        Err(crate::ferris::framework::types::Error::PermissionDenied(
            format!(
                "outbound http host `{host}` is not approved for app `{}`: {detail}",
                self.plugin_name
            ),
        ))
    }
}

/// Whether `host` matches a declared `allowed-hosts` entry: exact or
/// `*.suffix`, case-insensitive — the vocabulary `types.http-policy` documents
/// and the transport enforces. Duplicated from the transport's matcher because
/// this crate's host layer must not depend on the server-side HTTP stack.
fn declared_host(declared: &[String], host: &str) -> bool {
    let host = host.to_lowercase();
    declared.iter().any(|entry| {
        let entry = entry.trim().to_lowercase();
        entry.strip_prefix("*.").map_or_else(
            || host == entry,
            |suffix| host == suffix || host.ends_with(&format!(".{suffix}")),
        )
    })
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
        let mut host = HostState::new(std::sync::Arc::new(NullHost));
        host.set_plugin_name("demo");
        host
    }

    fn approval(capabilities: &[&str], http_hosts: &[&str]) -> crate::trust::GrantSet {
        crate::trust::GrantSet::new(
            capabilities
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            http_hosts
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            Vec::new(),
        )
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

    /// A specific capability needs both halves: the manifest request and the
    /// operator approval.
    #[test]
    fn a_declared_specific_capability_is_granted() {
        let mut host = host();
        host.set_grants(GrantSet::for_declaration([Capability::TlsClient]));
        host.set_approved_grants(Some(approval(&["tls-client"], &[])));
        assert!(host.require(Capability::TlsClient).is_ok());
        assert!(host.require(Capability::TlsServer).is_err());
    }

    /// The manifest capability alone is not a grant: an approval with empty
    /// lists withholds it.
    #[test]
    fn a_manifest_capability_with_empty_grants_is_denied() {
        let mut host = host();
        host.set_grants(GrantSet::for_declaration([Capability::Process]));
        host.set_approved_grants(Some(approval(&[], &[])));
        let error = host
            .require(Capability::Process)
            .expect_err("the operator approved nothing");
        match error {
            crate::ferris::framework::types::Error::PermissionDenied(message) => {
                assert!(message.contains("`process`"), "{message}");
                assert!(message.contains("not approved"), "{message}");
            }
            other => panic!("expected permission-denied, got {other:?}"),
        }
    }

    /// The approval unlocks a requested capability; the manifest still has to
    /// request it, so the other order remains denied.
    #[test]
    fn an_approved_capability_requires_the_manifest_request_too() {
        let mut host = host();
        host.set_approved_grants(Some(approval(&["process"], &[])));
        assert!(
            host.require(Capability::Process).is_err(),
            "approval without a manifest request is not a grant"
        );
        host.set_grants(GrantSet::for_declaration([Capability::Process]));
        assert!(host.require(Capability::Process).is_ok());
    }

    /// Missing `grants.toml`: non-primordial capabilities fail closed with the
    /// app and the files the host looked for; primordial ones keep working.
    #[test]
    fn a_missing_grants_file_denies_a_specific_capability() {
        let mut host = host();
        host.set_grants(GrantSet::for_declaration([Capability::Process]));
        host.set_approved_grants(None);
        let error = host
            .require(Capability::Process)
            .expect_err("no approval file means nothing is approved");
        match error {
            crate::ferris::framework::types::Error::PermissionDenied(message) => {
                assert!(message.contains("app `demo`"), "{message}");
                assert!(message.contains("grants.toml"), "{message}");
                assert!(message.contains("`process`"), "{message}");
            }
            other => panic!("expected permission-denied, got {other:?}"),
        }
        assert!(
            host.require(Capability::Http).is_ok(),
            "primordial capabilities keep the pre-trust rule"
        );
    }

    /// The outbound allow-list intersects the manifest's declared hosts with
    /// the operator approval: neither half can widen the other.
    #[test]
    fn an_http_host_needs_the_declaration_and_the_approval() {
        let mut host = host();
        let declared = vec!["manifest.test".to_string()];
        host.set_approved_grants(Some(approval(&["http"], &["approved.test"])));

        let error = host
            .require_http_host("approved.test", &declared)
            .expect_err("approved by the operator but outside the manifest declaration");
        match error {
            crate::ferris::framework::types::Error::PermissionDenied(message) => {
                assert!(message.contains("declared `allowed-hosts`"), "{message}");
            }
            other => panic!("expected permission-denied, got {other:?}"),
        }

        let error = host
            .require_http_host("manifest.test", &declared)
            .expect_err("declared by the app but not approved by the operator");
        match error {
            crate::ferris::framework::types::Error::PermissionDenied(message) => {
                assert!(message.contains("not approved"), "{message}");
                assert!(message.contains("`manifest.test`"), "{message}");
            }
            other => panic!("expected permission-denied, got {other:?}"),
        }

        host.set_approved_grants(Some(approval(&["http"], &["manifest.test"])));
        assert!(
            host.require_http_host("manifest.test", &declared).is_ok(),
            "both halves name the host"
        );
        assert!(
            host.require_http_host("manifest.test", &[]).is_ok(),
            "an empty declaration is no narrowing, not a denial"
        );
    }

    /// A missing approval denies outbound HTTP too, naming the app and the
    /// missing file.
    #[test]
    fn a_missing_grants_file_denies_an_http_host() {
        let mut host = host();
        host.set_approved_grants(None);
        let error = host
            .require_http_host("api.test", &[])
            .expect_err("no approval file");
        match error {
            crate::ferris::framework::types::Error::PermissionDenied(message) => {
                assert!(message.contains("app `demo`"), "{message}");
                assert!(message.contains("grants.toml"), "{message}");
            }
            other => panic!("expected permission-denied, got {other:?}"),
        }
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
