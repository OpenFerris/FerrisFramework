//! Reading what the loaded apps declare: `describe`, its JSON projection and
//! the API method list.

use anyhow::Result;

use super::registry::PluginStore;
use crate::descriptor::{self, MethodSpec};
use crate::ferris::framework::descriptor::Descriptor;

impl PluginStore {
    /// What the loaded app advertises (`describe`), as the typed WIT
    /// `descriptor` record — the same values `handle-request`/the registries
    /// consume.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded or the guest's `describe`
    /// call fails.
    pub fn describe(&self) -> Result<Descriptor> {
        let Some(plugin) = self.loaded() else {
            anyhow::bail!("no plugin loaded");
        };
        self.describe_of(plugin.name())
    }

    /// `describe()` of the app registered under `name` (SRV17).
    ///
    /// # Errors
    ///
    /// Returns an error when no app is registered under `name` or its
    /// `describe()` call fails.
    pub fn describe_of(&self, name: &str) -> Result<Descriptor> {
        let Some(plugin) = self.get(name) else {
            anyhow::bail!("plugin `{name}` is not loaded");
        };
        let res = plugin.describe();
        self.guard_trap(res, name).map_err(anyhow::Error::from)
    }

    /// The `describe` descriptor projected onto the stable JSON surface the
    /// dev endpoint `/plugins/{name}` serves. Built host-side from the typed
    /// record — not by echoing the guest's payload back.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded, the guest's `describe` call
    /// fails, or the projection cannot be serialized.
    pub fn describe_json(&self) -> Result<String> {
        let described = self.describe()?;
        serde_json::to_string(&descriptor::DescriptorJson::from(&described))
            .map_err(|e| anyhow::anyhow!("failed to serialize describe() projection: {e}"))
    }

    /// `describe_json()` of the app registered under `name` (SRV17).
    ///
    /// # Errors
    ///
    /// Returns an error when no app is registered under `name` or its
    /// `describe()` call fails.
    pub fn describe_json_for(&self, name: &str) -> Result<String> {
        let described = self.describe_of(name)?;
        serde_json::to_string(&descriptor::DescriptorJson::from(&described))
            .map_err(|e| anyhow::anyhow!("failed to serialize describe() projection: {e}"))
    }

    /// The API methods the loaded app declares in `describe()`, ready for the
    /// host to register on `/api/method/<name>`.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded or the guest's `describe`
    /// call fails.
    pub fn methods(&self) -> Result<Vec<MethodSpec>> {
        let described = self.describe()?;
        Ok(described.methods.iter().map(MethodSpec::from).collect())
    }

    /// The network surface the loaded app declared: routes, capabilities, the
    /// default auth and the HTTP/TLS policy, as stored when `describe()` last
    /// succeeded.
    ///
    /// Nothing here binds anything — the server mounts what this returns — and
    /// a plugin that has not described itself yet yields an error instead of an
    /// empty surface, so a router cannot silently mount nothing.
    ///
    /// # Errors
    ///
    /// Returns an error when no plugin is loaded or it has not described itself.
    pub fn network(&self) -> Result<crate::descriptor::NetworkSpec> {
        let Some(plugin) = self.loaded() else {
            anyhow::bail!("no plugin loaded");
        };
        self.network_for(plugin.name())
    }

    /// [`network`](Self::network) of the app registered under `name`.
    ///
    /// # Errors
    ///
    /// Returns an error when no app is registered under `name` or it has not
    /// described itself yet.
    pub fn network_for(&self, name: &str) -> Result<crate::descriptor::NetworkSpec> {
        let Some(plugin) = self.get(name) else {
            anyhow::bail!("plugin `{name}` is not loaded");
        };
        plugin
            .network()
            .ok_or_else(|| anyhow::anyhow!("plugin `{name}` has not described itself yet"))
    }

    /// The API methods the app registered under `name` declares (SRV17).
    ///
    /// # Errors
    ///
    /// Returns an error when no app is registered under `name` or the guest's
    /// `describe()` call fails.
    pub fn methods_for(&self, name: &str) -> Result<Vec<MethodSpec>> {
        let described = self.describe_of(name)?;
        Ok(described.methods.iter().map(MethodSpec::from).collect())
    }
}
