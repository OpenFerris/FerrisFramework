//! The store itself: the app registry, its atomic swap and the shared runtime.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use anyhow::Result;
use parking_lot::RwLock;
use tracing::{info, warn};

use crate::Plugin;
use crate::host_impl::RealHostState;
use crate::runtime::SharedRuntime;
use ferris_db::Db;

/// The app plugins the host serves, keyed by name and atomically swappable.
///
/// A multi-app install registers every loaded component: `/plugins/{name}`,
/// method dispatch and UI assets resolve the app that declared them instead of
/// only the most recently loaded one (SRV17). The single-app accessors
/// ([`Self::loaded`], [`Self::describe`], [`Self::methods`]) keep addressing
/// the most recent swap for backwards compatibility.
pub struct PluginStore {
    pub(super) runtime: Arc<SharedRuntime>,
    pub(super) pool: Db,
    /// Name → live instance.
    pub(super) plugins: RwLock<HashMap<String, Arc<Plugin>>>,
    /// Name of the most recently swapped app; the default for the name-less
    /// accessors (doctype events, jobs, `describe`).
    pub(super) current: RwLock<Option<String>>,
    /// `DocTypes` already persisted and hooked into the engine, keyed by
    /// `(app, doctype)` and mapped to the fingerprint of the meta they were
    /// registered from (so a `DocType` edited while the dev server runs
    /// re-registers on the next reload). The app is part of the key: two apps
    /// declaring the same name must not silently skip one another.
    pub(super) synced: Mutex<HashMap<(String, String), String>>,
    /// `doctype_event`s that hit the instance while busy, awaiting the
    /// background drainer.
    pub(super) pending: parking_lot::Mutex<VecDeque<super::events::QueuedEvent>>,
    /// Wakes the drainer the moment an event is queued (set once).
    pub(super) wake: OnceLock<mpsc::Sender<()>>,
    /// Component bytes per app, so a trapped instance can be replaced by a
    /// fresh one ([`Self::recover_after_trap`]). Absent for a component that
    /// arrived through a path that did not go through the store.
    pub(super) sources: Mutex<HashMap<String, Vec<u8>>>,
    /// Directory that holds every app world (`plugins/`); `None` for a store
    /// that hosts no apps (tools, tests).
    pub(super) worlds: Option<std::path::PathBuf>,
    /// Surface keys (`doctype:<name>`, `job:<type>`) already reported as
    /// unowned or ambiguous, so the event path logs once per name instead of
    /// once per document. Cleared on a swap: a reload can give the surface an
    /// owner, and the new situation deserves its own line.
    pub(super) noticed: parking_lot::Mutex<HashSet<String>>,
}

impl PluginStore {
    /// Create an empty store over a fresh manager bound to the shared runtime.
    #[must_use]
    pub fn new(runtime: Arc<SharedRuntime>, pool: Db) -> Self {
        Self {
            runtime,
            pool,
            plugins: RwLock::new(HashMap::new()),
            current: RwLock::new(None),
            synced: Mutex::new(HashMap::new()),
            pending: parking_lot::Mutex::new(VecDeque::new()),
            wake: OnceLock::new(),
            sources: Mutex::new(HashMap::new()),
            worlds: None,
            noticed: parking_lot::Mutex::new(HashSet::new()),
        }
    }

    /// Point the store at the directory that holds every app world
    /// (`plugins/`), so each instance is mounted on its own sub-directory.
    ///
    /// Without it no world is mounted: a unit test or a tool that builds a
    /// store has no project directory, and inventing one under the process's
    /// cwd would scatter `plugins/<name>/` behind the caller's back.
    #[must_use]
    pub fn with_worlds(mut self, worlds: impl Into<std::path::PathBuf>) -> Self {
        self.worlds = Some(worlds.into());
        self
    }

    /// The shared runtime backing this store.
    pub fn runtime(&self) -> &SharedRuntime {
        &self.runtime
    }

    /// The world directory of `name`, when the store was given a worlds root
    /// and the name is a safe path segment.
    ///
    /// This is the host's side of the same directory the guest sees at
    /// `/world`; the server uses it for host-resolved material (TLS PEM) and
    /// for nothing the guest could reach directly.
    #[must_use]
    pub fn world_of(&self, name: &str) -> Option<std::path::PathBuf> {
        let root = self.worlds.as_ref()?;
        ferris_core::plugins::world_root_at(root, name).ok()
    }

    /// The database the host shares with the API, for host-side lookups that
    /// belong to the same tenant (the `Widget` `DocType` row's stored code).
    #[must_use]
    pub fn pool(&self) -> &Db {
        &self.pool
    }

    /// Start the shared runtime's epoch clock, if epoch interrupts are enabled.
    ///
    /// §C4: the host arms an epoch deadline around every plugin call
    /// (`Plugin::invoke`), but a deadline only fires when a background clock
    /// keeps bumping the engine's epoch. Before C4 no production path started
    /// that clock — `spawn_epoch_clock` was only exercised from tests — so a
    /// plugin in an infinite loop hung its worker forever while looking
    /// protected. `ferris_server::Server::run` calls this at boot for every
    /// store the operator configures. Each call detaches one clock task; call
    /// it once per store, at boot. Returns `None` when interrupts are off.
    pub fn start_epoch_clock(&self) -> Option<tokio::task::JoinHandle<()>> {
        self.runtime.spawn_epoch_clock()
    }

    /// Swap in an already-instantiated plugin (e.g. pre-warmed before the
    /// router was built). Other apps already registered keep serving; a plugin
    /// with the same name is replaced.
    pub fn swap(&self, plugin: Arc<Plugin>) {
        let name = plugin.name().to_string();
        info!(plugin = %name, "hot-reloaded plugin swapped in");
        self.plugins.write().insert(name.clone(), plugin);
        *self.current.write() = Some(name);
        // A reload can change which surfaces have an owner, so the warn-once
        // bookkeeping for the previous one no longer describes the install.
        self.noticed.lock().clear();
    }

    /// The app registered under `name`, if any (SRV17).
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<Plugin>> {
        self.plugins.read().get(name).cloned()
    }

    /// Every registered app, **stable-ordered by name**.
    ///
    /// Registration walks this list (methods, doctype wiring, plugins), and a
    /// `HashMap` iteration order is not stable across processes. Sorting makes
    /// the order deterministic: two boot runs of the same install register the
    /// same app first, so a name collision is reproducible instead of a
    /// race with whichever `HashMap` order the process happened to build.
    #[must_use]
    pub fn plugins(&self) -> Vec<Arc<Plugin>> {
        let mut plugins: Vec<Arc<Plugin>> = self.plugins.read().values().cloned().collect();
        plugins.sort_by(|a, b| a.name().cmp(b.name()));
        plugins
    }

    /// The most recently swapped plugin, if any.
    pub fn loaded(&self) -> Option<Arc<Plugin>> {
        let name = self.current.read().clone()?;
        self.get(&name)
    }

    /// Name of the loaded plugin, if any.
    pub fn name(&self) -> Option<String> {
        self.loaded().map(|p| p.name().to_string())
    }

    /// Compile a component into a plugin without publishing it.
    pub(super) fn compile(&self, bytes: &[u8], name: &str) -> Result<Arc<Plugin>> {
        let mut state = RealHostState::for_pool(self.pool.clone());
        state.set_worlds_root(self.worlds.clone());
        Ok(Arc::new(crate::Plugin::from_bytes(
            &self.runtime,
            state,
            name.to_string(),
            bytes,
        )?))
    }

    /// Keep the bytes of the app registered under `name`, so a trapped instance
    /// can be rebuilt by [`Self::recover_after_trap`] (SRV17: one source per
    /// app, not just the last one loaded).
    ///
    /// Copying on every reload is deliberate: the alternative — re-reading the
    /// file at recovery time — would rebuild from whatever is on disk *then*,
    /// which may have changed or been deleted while the server runs.
    pub(super) fn remember(&self, bytes: &[u8], name: &str) {
        let mut sources = self
            .sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sources.insert(name.to_string(), bytes.to_vec());
    }
}

/// Ownership: which loaded app declared a surface.
///
/// Lifecycle events and background jobs used to address [`PluginStore::loaded`]
/// (the app swapped in last). With N apps that sent one app the JSON of another
/// app's document, left the owner unable to validate its own writes, and let
/// the last-loaded app veto writes it does not own (P0-6/WASM-003/SEC-05), while
/// `enqueue-job` accepted any `job_type` at all (WASM-007/SEC-11). Dispatch now
/// resolves the owner first and addresses that app only; an unowned surface is
/// not delivered to anybody.
impl PluginStore {
    /// The app that declared `doctype`, or `None` when no loaded app did.
    ///
    /// The declaration record is `synced`: [`PluginStore::register_doctypes_of`]
    /// writes one `(app, doctype)` entry per `DocType` it actually registered
    /// from the app's `describe()`, so ownership is the wiring the engine's
    /// hooks were built from — no extra guest call, and therefore safe to
    /// resolve from inside a document write.
    ///
    /// The schema registry already refuses the same name to a second app, so a
    /// collision only reaches this point from a stale record; when it does, the
    /// **first app by name** wins and the event is delivered once, deterministically,
    /// instead of to whichever app the map iterated last.
    ///
    /// An install with a **single** loaded app keeps the historical addressing:
    /// with one app there is no second app to leak a document to, so an
    /// undeclared `DocType` (a framework one, say) is still that app's. With two
    /// or more apps an undeclared name has no owner and reaches nobody — the
    /// cross-app leak this lookup exists to close.
    #[must_use]
    pub fn owner_of_doctype(&self, doctype: &str) -> Option<String> {
        let synced = self
            .synced
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut owner: Option<&str> = None;
        for (app, declared) in synced.keys() {
            if declared != doctype {
                continue;
            }
            match owner {
                Some(current) if current <= app.as_str() => {
                    if current != app.as_str()
                        && self.notice_once(format!("doctype-conflict:{doctype}"))
                    {
                        warn!(
                            doctype,
                            kept = current,
                            ignored = app.as_str(),
                            "two loaded apps declare the same DocType; events address the first by name"
                        );
                    }
                }
                _ => owner = Some(app.as_str()),
            }
        }
        let owner = owner.map(str::to_string);
        drop(synced);
        owner.or_else(|| match self.plugins().as_slice() {
            [only] => Some(only.name().to_string()),
            _ => None,
        })
    }

    /// The plugin instance that owns `doctype`, when it is still registered.
    ///
    /// A name in the declaration record whose instance has gone away is an
    /// unowned surface, not a reason to fall back to another app.
    #[must_use]
    pub fn owner_plugin(&self, doctype: &str) -> Option<Arc<Plugin>> {
        self.owner_of_doctype(doctype)
            .and_then(|owner| self.get(&owner))
    }

    /// The app that owns `job_type`, or `None` when the type is ambiguous.
    ///
    /// The WIT contract documents the app spelling `<app>.<job>`
    /// (`app.monthly`), and that prefix is the only name shape that survives
    /// more than one loaded app, so it is checked first. The descriptor has no
    /// job list to consult, so with a **single** loaded app every job type is
    /// unambiguously that app's (the pre-existing behaviour, now explicit
    /// instead of "whichever app loaded last"). With two or more apps an
    /// unprefixed or foreign-prefixed type has no owner and is refused.
    #[must_use]
    pub fn owner_of_job(&self, job_type: &str) -> Option<String> {
        let job_type = job_type.trim();
        if job_type.is_empty() {
            return None;
        }
        if let Some((prefix, rest)) = job_type.split_once('.')
            && !prefix.is_empty()
            && !rest.is_empty()
            && self.get(prefix).is_some()
        {
            return Some(prefix.to_string());
        }
        let plugins = self.plugins();
        match plugins.as_slice() {
            [only] => Some(only.name().to_string()),
            _ => None,
        }
    }

    /// Record a surface as reported and tell the caller whether this is the
    /// first report *for that key*, so an event storm cannot flood the log.
    pub(super) fn notice_once(&self, key: String) -> bool {
        self.noticed.lock().insert(key)
    }
}

impl std::fmt::Debug for PluginStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginStore")
            .field("current", &self.name())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::PluginStore;
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

    /// Record an app `DocType` the way the wiring path does, without a guest:
    /// `register_doctypes_of` writes exactly this `(app, doctype)` entry.
    fn declare(store: &PluginStore, app: &str, doctype: &str) {
        store
            .synced
            .lock()
            .expect("synced")
            .insert((app.to_string(), doctype.to_string()), "fp".to_string());
    }

    /// P0-6/SEC-05: the owner is the app that registered the `DocType`, so a
    /// lifecycle event can address it instead of `loaded()`.
    #[tokio::test]
    async fn owner_of_doctype_resolves_the_declaring_app() {
        let store = store().await;
        declare(&store, "b-app", "Order");
        declare(&store, "a-app", "Invoice");
        assert_eq!(store.owner_of_doctype("Order").as_deref(), Some("b-app"));
        assert_eq!(store.owner_of_doctype("Invoice").as_deref(), Some("a-app"));
        assert_eq!(store.owner_of_doctype("Unrelated"), None);
    }

    /// A collision is resolved the same way on every boot: first app by name.
    /// The schema registry already refuses a name to a second app, so this only
    /// fires from a stale record — determinism still matters.
    #[tokio::test]
    async fn owner_of_doctype_prefers_the_first_app_by_name_on_a_collision() {
        let store = store().await;
        declare(&store, "z-app", "Order");
        declare(&store, "a-app", "Order");
        assert_eq!(store.owner_of_doctype("Order").as_deref(), Some("a-app"));
    }

    /// A record whose instance is gone is unowned; it must not fall back to
    /// another app's instance.
    #[tokio::test]
    async fn owner_plugin_is_none_when_the_owning_app_is_not_registered() {
        let store = store().await;
        declare(&store, "a-app", "Invoice");
        assert_eq!(store.owner_of_doctype("Invoice").as_deref(), Some("a-app"));
        assert!(store.owner_plugin("Invoice").is_none());
    }

    /// A store with no app owns nothing: dispatch must not invent an owner.
    #[tokio::test]
    async fn an_empty_install_owns_no_surface() {
        let store = store().await;
        assert_eq!(store.owner_of_doctype("Invoice"), None);
        assert_eq!(store.owner_of_job("a.nightly"), None);
        assert_eq!(store.owner_of_job("task_nightly"), None);
    }
}
