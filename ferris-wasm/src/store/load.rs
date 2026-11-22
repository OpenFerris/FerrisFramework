//! Publishing a component: load, wire, then swap.

use std::sync::Arc;

use anyhow::Result;

use super::registry::PluginStore;
use crate::Plugin;

impl PluginStore {
    /// Load a `*.wasm` component and atomically replace the current plugin.
    ///
    /// On failure the previous plugin is kept and the error returned. Returns
    /// the fresh plugin on success.
    ///
    /// # Errors
    ///
    /// Returns an error when the component fails to parse, compile, or
    /// instantiate (the previous plugin stays loaded).
    pub fn swap_load(&self, path: impl AsRef<std::path::Path>) -> Result<Arc<Plugin>> {
        let bytes = std::fs::read(&path)?;
        // One path for both loads: a store that knows its worlds root must not
        // lose it on the name-less convenience constructor.
        self.swap_load_bytes(&bytes, "plugin")
    }

    /// Load a component with a specific plugin name (used by `ferris dev`).
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or the component fails to
    /// compile/instantiate.
    pub fn swap_load_with_name(
        &self,
        path: impl AsRef<std::path::Path>,
        name: &str,
    ) -> Result<Arc<Plugin>> {
        let bytes = std::fs::read(&path)?;
        self.swap_load_bytes(&bytes, name)
    }

    /// Load a component already in memory and atomically replace the current
    /// plugin, without touching the schema registry or the engine hooks.
    ///
    /// Hot reloads want [`Self::swap_load_and_wire`] instead: this one leaves the
    /// `DocTypes` of the new component unregistered.
    ///
    /// On failure the previous plugin is kept and the error returned. Returns
    /// the fresh plugin on success.
    ///
    /// # Errors
    ///
    /// Returns an error when the component fails to compile or instantiate.
    pub fn swap_load_bytes(&self, bytes: &[u8], name: &str) -> Result<Arc<Plugin>> {
        self.remember(bytes, name);
        let plugin = self.compile(bytes, name)?;
        self.swap(plugin.clone());
        Ok(plugin)
    }

    /// Compile a component, wire what it declares, and only then publish it.
    ///
    /// On any failure the previously loaded plugin stays in place and is the one
    /// serving requests. This is the path every hot reload takes: swapping first
    /// and wiring afterwards left a window where the new component was live but
    /// its `DocTypes` were neither in the registry nor hooked into the engine.
    ///
    /// # Errors
    ///
    /// Returns an error when the component fails to compile or wire, or the
    /// engine rejects a declared `DocType`.
    pub fn swap_load_and_wire(self: &Arc<Self>, bytes: &[u8], name: &str) -> Result<Arc<Plugin>> {
        self.remember(bytes, name);
        let plugin = self.compile(bytes, name)?;
        self.start_drainer();
        self.wire_events_of(&plugin)?;
        self.swap(plugin.clone());
        Ok(plugin)
    }
}
