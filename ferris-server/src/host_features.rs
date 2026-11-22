//! The host feature set: what this `ferris-server` build can actually serve.
//!
//! Cargo decides features at compile time; the WIT layer
//! (`platform::features()`), `ferris doctor` and the boot-time check of
//! `[features]` all need to know the answer at run time. There is exactly one
//! source of truth — [`compiled`], built from `cfg!(feature = …)` in this file —
//! and it is published through [`ferris_core::host_features`] so `ferris-wasm`
//! and `ferris-cli` read the same set without either depending on the other
//! (this crate depends on both, so it cannot be the storage location without a
//! cycle).
//!
//! Keep [`FEATURES`] in step with the `[features]` table of
//! `ferris-server/Cargo.toml` and `ferris-cli/Cargo.toml`; the doctor prints the
//! names verbatim, so a rename is a wire change.

use ferris_core::host_features::{self as registry, HostFeatures};

/// Every feature this crate can be built with, in the order the documentation
/// and `ferris doctor` present them.
///
/// `full` is included on purpose: it is a reported capability ("all of the
/// above"), not only an alias.
pub const FEATURES: [&str; 12] = [
    "api",
    "web",
    "realtime",
    "jobs",
    "scheduler",
    "email",
    "files",
    "print",
    "report",
    "integrations",
    "wasm",
    "full",
];

/// The verticals a `full` build contains. `api` is the base and is not listed:
/// it is not a vertical.
const VERTICALS: [&str; 10] = [
    "web",
    "realtime",
    "jobs",
    "scheduler",
    "email",
    "files",
    "print",
    "report",
    "integrations",
    "wasm",
];

/// Whether `name` was compiled into this binary.
///
/// An unknown name is `false`: a capability question must not answer "yes" for
/// a feature nobody declared.
#[must_use]
pub fn is_compiled(name: &str) -> bool {
    match name {
        "api" => true,
        "web" => cfg!(feature = "web"),
        "realtime" => cfg!(feature = "realtime"),
        "jobs" => cfg!(feature = "jobs"),
        "scheduler" => cfg!(feature = "scheduler"),
        "email" => cfg!(feature = "email"),
        "files" => cfg!(feature = "files"),
        "print" => cfg!(feature = "print"),
        "report" => cfg!(feature = "report"),
        "integrations" => cfg!(feature = "integrations"),
        "wasm" => cfg!(feature = "wasm"),
        // `full` is a union, not an independent switch: it reports `true` only
        // when every vertical is present, which is what a caller
        // branching on it means.
        "full" => VERTICALS.iter().all(|vertical| is_compiled(vertical)),
        _ => false,
    }
}

/// The compiled feature set of this binary.
#[must_use]
pub fn compiled() -> HostFeatures {
    HostFeatures::new(FEATURES.into_iter().filter(|name| is_compiled(name)))
}

/// Publish [`compiled`] process-wide and return whether this call installed it.
///
/// Called once, early in `Server::run`, before anything can ask a capability
/// question (the WIT import and the health/doctor surfaces). First install wins,
/// exactly like the realtime handler and the PKI provider: a second boot in the
/// same process must not advertise a different set than the code running.
#[must_use]
pub fn install_compiled() -> bool {
    registry::install(compiled())
}

/// The installed set, falling back to [`compiled`] when nothing was installed
/// (unit tests, an embedder that only calls `build_router`).
#[must_use]
pub fn installed() -> HostFeatures {
    registry::installed().cloned().unwrap_or_else(compiled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_is_always_compiled_and_unknown_names_never_are() {
        assert!(is_compiled("api"));
        assert!(!is_compiled("nonsense"));
        assert!(!is_compiled(""));
    }

    #[test]
    fn the_compiled_set_reports_the_features_this_test_build_enabled() {
        let features = compiled();
        // `api` is the base every build has.
        assert!(features.contains("api"));
        // Everything else mirrors `cfg!`, so the assertion is the same decision
        // the rest of the crate takes.
        for name in FEATURES {
            assert_eq!(
                features.contains(name),
                is_compiled(name),
                "{name} must be reported exactly as compiled"
            );
        }
    }

    #[test]
    fn full_is_the_union_of_every_vertical() {
        let all = VERTICALS.iter().all(|name| is_compiled(name));
        assert_eq!(is_compiled("full"), all);
    }

    #[test]
    fn install_falls_back_to_the_compiled_set() {
        // Either a set was installed (this binary's own boot) or the fallback
        // answers the same names as `compiled`.
        assert!(!installed().is_empty());
    }
}
