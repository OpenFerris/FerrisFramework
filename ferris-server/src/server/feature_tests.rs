//! The compile-time feature matrix, pinned across the three layers that have to
//! agree on it.
//!
//! A subsystem exists at three levels and each one can drift:
//!
//! 1. the **Cargo feature** (`cfg!(feature = …)`, reported by
//!    [`crate::host_features`]);
//! 2. the **role** that starts it ([`ServerRole`] + `gate_roles`);
//! 3. the **run-time switch** the operator writes ([`FeatureFlags`]).
//!
//! These tests assert the joins rather than exhaustively restating the table:
//! every role names a feature that exists, every flag that has a Cargo feature
//! has a role-or-route gate, and the defaults leave every vertical on.

use ferris_core::FeatureFlags;

use crate::host_features;
use crate::roles::{ServerRole, ServerRoles};

/// Every role names a declared Cargo feature, and `api` is the only base role.
#[test]
fn every_role_names_a_declared_feature() {
    for role in ServerRole::ALL {
        let feature = role.cargo_feature();
        assert!(
            host_features::FEATURES.contains(&feature),
            "role {role} names `{feature}`, which is not a declared feature"
        );
    }
    assert_eq!(ServerRole::Api.cargo_feature(), "api");
    assert_eq!(ServerRole::Worker.cargo_feature(), "jobs");
}

/// The role list is the wire/CLI surface and must not depend on the build: a
/// config file parses identically on every binary.
#[test]
fn the_role_names_are_stable_across_builds() {
    let names: Vec<&str> = ServerRole::ALL.iter().map(ServerRole::as_str).collect();
    assert_eq!(names, vec!["api", "web", "realtime", "worker", "scheduler"]);
    for name in &names {
        assert_eq!(
            name.parse::<ServerRole>()
                .expect("every role parses")
                .as_str(),
            *name
        );
    }
}

/// `[features]` defaults are every vertical on. A default config therefore
/// boots on a default build.
#[test]
fn feature_flags_default_to_every_vertical() {
    let flags = FeatureFlags::default();
    assert!(flags.realtime);
    assert!(flags.jobs);
    assert!(flags.scheduler);
    assert!(flags.email);
    assert!(flags.files);
    assert!(flags.print);
    assert!(flags.report);
    assert!(flags.search);
    assert!(flags.webhooks);
}

/// A `web`-less build still has the api role, and `all` never grows a role.
#[test]
fn all_never_grows_a_role_beyond_the_compiled_set() {
    let flags = FeatureFlags::default();
    let gated = super::gate_roles(&ServerRoles::all(), &flags).unwrap();
    assert!(gated.has(ServerRole::Api));
    for role in gated.iter() {
        assert!(
            host_features::is_compiled(role.cargo_feature()),
            "role {role} survived without `{}`",
            role.cargo_feature()
        );
    }
}
