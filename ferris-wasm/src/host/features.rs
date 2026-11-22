//! Feature detection across the plugin boundary: what the **running** host was
//! compiled with.
//!
//! Cargo features are compile-time, so a prebuilt `ferris` binary cannot gain a
//! vertical at runtime. The WIT contract, however, is static: every optional
//! interface stays declared, and the guest has to learn somehow which of them
//! this host actually has. That is the one job of this module — it is the
//! WIT-side equivalent of `#ifdef`, surfaced as `platform::features()`.
//!
//! **There is exactly one feature set in the process**, and this module does not
//! own it: [`ferris_core::host_features`] is the registry, `ferris-server` fills
//! it from `cfg!(feature = …)` at boot
//! (`ferris_server::host_features::install_compiled`), and `ferris doctor` prints
//! the same value. Duplicating the slot here would give two answers to one
//! question — the defect this crate is most careful about — so this file is an
//! adapter, not a second registry:
//!
//! - `ferris-server` depends on this crate, so the storage cannot live here;
//! - `ferris-core` is the bottom of the graph, so it can be read by both.
//!
//! The fallback follows the registry's own reading: a process that never
//! installed a set (unit tests, an embedder that only built a router) reports an
//! **empty** list, never a guessed one. Understating is safe — a guest hides an
//! affordance it might have had — while overstating sends the guest into an
//! import that answers `unsupported`.

/// The process-wide compiled-feature set, as `ferris-core` defines it.
pub use ferris_core::host_features::HostFeatures;

/// What `platform::features()` answers: the installed set, or an empty one when
/// nothing was installed.
///
/// Read this rather than `cfg!(…)`: the runtime is one crate compiled once, while
/// the verticals are features of the **server** (and of the CLI) that embed it.
#[must_use]
pub fn host_features() -> HostFeatures {
    ferris_core::host_features::installed()
        .cloned()
        .unwrap_or_default()
}

/// Whether the host was compiled with `feature`.
///
/// Same rule as [`host_features`]: an uninstalled process answers `false`, which
/// is the safe reading for a capability question.
#[must_use]
pub fn host_has_feature(feature: &str) -> bool {
    ferris_core::host_features::is_enabled(feature)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing installed means "nothing known": the guest must hide the optional
    /// surfaces rather than call a service that would answer `unsupported`.
    ///
    /// `install` is process-global, so this only pins the fallback when no test
    /// in this binary has installed a set.
    #[test]
    fn an_uninstalled_host_reports_nothing() {
        if ferris_core::host_features::installed().is_some() {
            return;
        }
        assert!(host_features().is_empty());
        assert!(!host_has_feature("api"));
    }

    /// The set is read, not copied: a name installed by the server is visible
    /// here without this crate forwarding anything.
    #[test]
    fn the_reported_set_comes_from_the_core_registry() {
        if let Some(installed) = ferris_core::host_features::installed() {
            assert_eq!(host_features().to_vec(), installed.to_vec());
            assert_eq!(host_has_feature("api"), installed.contains("api"));
        }
    }
}
