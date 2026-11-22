//! Hot-swappable plugin store backing `ferris dev`.
//!
//! Split by concern: `registry` owns the store struct and the name → instance
//! map, `load` publishes a component atomically, `describe` reads what an app
//! declares, `requests` forwards HTTP/stream/job/UI calls, `doctypes` wires the
//! declared schema and lifecycle hooks, `events` owns the deferred event queue,
//! `hooks` adapts a store call to an engine hook, and `recovery` rebuilds an
//! instance that trapped.

mod describe;
mod doctypes;
mod events;
mod hooks;
mod load;
mod recovery;
mod registry;
mod requests;

pub use registry::PluginStore;

#[cfg(test)]
mod tests;
