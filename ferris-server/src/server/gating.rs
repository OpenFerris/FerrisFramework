//! `[features]` gating: run-time switches checked against the compiled set.
//!
//! Two different questions live here, and conflating them is what produced the
//! "silently served without it" bugs this module exists to prevent:
//!
//! * [`gate_roles`] — *may this process start this role?* A `[features]` flag of
//!   `false` and a Cargo feature that was not compiled both answer "no", with a
//!   log line naming which of the two it was.
//! * [`validate_compiled_features`] — *does the operator's config ask for
//!   something this binary cannot do?* That is a **boot error**: refusing to
//!   start is the only honest response, because the alternative is a process
//!   that reports itself healthy while serving less than its configuration
//!   declares.
//!
//! The two layers are separate on purpose (AGENTS §2): a compiled-out role is a
//! deployment fact and is reported once, while a config that contradicts the
//! binary is an operator mistake and stops the boot.

use anyhow::Result;
use ferris_core::{Config, FeatureFlags};
use tracing::warn;

use crate::host_features;
use crate::roles::{ServerRole, ServerRoles};

/// Remove the roles whose subsystem the operator disabled in `[features]` or
/// this binary was not built with.
///
/// `false` is authoritative even for `--all` or `roles = ["all"]`, so a process
/// cannot start a subsystem the configuration turned off. Each removal is
/// logged with the concrete reason: a run-time flag, or a missing Cargo feature
/// (which the operator fixes by rebuilding, not by editing the config).
/// `true` never adds a role that was not requested.
///
/// # Errors
///
/// Fails only if the reduced list cannot be re-parsed, which cannot happen: it
/// is a subset of names that already parsed.
pub fn gate_roles(roles: &ServerRoles, features: &FeatureFlags) -> Result<ServerRoles> {
    let mut names = Vec::new();
    for role in roles.iter() {
        let enabled = match role {
            ServerRole::Realtime => features.realtime,
            ServerRole::Worker => features.jobs,
            ServerRole::Scheduler => features.scheduler,
            ServerRole::Api | ServerRole::Web => true,
        };
        if !enabled {
            warn!(
                "feature `{}` is disabled in [features]: role `{role}` will not start",
                role.cargo_feature()
            );
            continue;
        }
        if !host_features::is_compiled(role.cargo_feature()) {
            warn!(
                "role `{role}` will not start: this binary was compiled without the `{}` \
                 Cargo feature (rebuild with `--features {}`)",
                role.cargo_feature(),
                role.cargo_feature()
            );
            continue;
        }
        names.push(role.as_str().to_string());
    }
    ServerRoles::parse(&names)
}

/// Fail the boot when the config asks for a subsystem this binary lacks.
///
/// The check is deliberately narrow: it looks at the flags an operator can set
/// to `true` for a vertical and compares each one against
/// [`host_features::compiled`]. `search` and `webhooks` are not Cargo features
/// (they live inside the always-compiled `ferris-api` and
/// `ferris-integrations` runtimes), so they have nothing to compare against and
/// are not checked here.
///
/// # Errors
///
/// `anyhow::Error` listing every mismatch at once, so an operator fixes one
/// config instead of one flag per restart.
pub fn validate_compiled_features(config: &Config) -> Result<()> {
    let compiled = host_features::compiled();
    let mut mismatches: Vec<String> = Vec::new();

    let flags = [
        ("realtime", config.features.realtime),
        ("jobs", config.features.jobs),
        ("scheduler", config.features.scheduler),
        ("email", config.features.email),
        ("files", config.features.files),
        ("print", config.features.print),
        ("report", config.features.report),
    ];
    for (name, enabled) in flags {
        if enabled && !compiled.contains(name) {
            mismatches.push(format!("[features] {name} = true"));
        }
    }

    if mismatches.is_empty() {
        return Ok(());
    }

    let requested = mismatches.join(", ");
    anyhow::bail!(
        "the configuration asks for subsystems this binary was not compiled with: {requested}. \
         Compiled features: `{compiled}`. Either rebuild ferris with \
         `--features <name>` for each one, or set the matching `[features]` flag to false \
         in the configuration."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gated_names(roles: ServerRoles, features: &FeatureFlags) -> Vec<String> {
        gate_roles(&roles, features)
            .expect("a subset of parsed names always re-parses")
            .iter()
            .map(|role| role.as_str().to_string())
            .collect()
    }

    #[test]
    fn a_disabled_flag_removes_only_its_role() {
        let mut features = FeatureFlags::default();
        features.realtime = false;
        features.jobs = false;

        let names = gated_names(ServerRoles::all(), &features);
        assert!(!names.contains(&"realtime".to_string()));
        assert!(!names.contains(&"worker".to_string()));
        assert!(names.contains(&"api".to_string()));
        assert!(names.contains(&"web".to_string()));
        assert!(names.contains(&"scheduler".to_string()));
    }

    #[test]
    fn compiled_out_roles_are_removed_without_failing_the_boot() {
        // Whatever this build compiled, every surviving role has its feature.
        let features = FeatureFlags::default();
        let gated = gate_roles(&ServerRoles::all(), &features).unwrap();
        for role in gated.iter() {
            assert!(
                host_features::is_compiled(role.cargo_feature()),
                "role {role} survived without its feature"
            );
        }
    }

    #[test]
    fn enabling_every_flag_never_adds_a_role() {
        let features = FeatureFlags::default();
        let only_api = ServerRoles::parse(&["api".to_string()]).unwrap();
        assert_eq!(gated_names(only_api, &features), vec!["api".to_string()]);
    }

    #[test]
    fn a_matching_config_passes_validation() {
        // A config that gates every vertical the way this build was compiled
        // must pass; that is what makes a minimal deployment possible.
        let mut config = Config::default();
        config.features.realtime = host_features::is_compiled("realtime");
        config.features.jobs = host_features::is_compiled("jobs");
        config.features.scheduler = host_features::is_compiled("scheduler");
        config.features.email = host_features::is_compiled("email");
        config.features.files = host_features::is_compiled("files");
        config.features.print = host_features::is_compiled("print");
        config.features.report = host_features::is_compiled("report");
        assert!(validate_compiled_features(&config).is_ok());
    }

    #[test]
    fn a_feature_flag_for_a_compiled_out_vertical_is_named_in_the_error() {
        // The flags `validate_compiled_features` reads, in order. `web`,
        // `integrations`, `wasm` and `api` have no `[features]` flag: they are
        // compile-time only.
        const FLAGGED: [&str; 7] = [
            "realtime",
            "jobs",
            "scheduler",
            "email",
            "files",
            "print",
            "report",
        ];
        let Some(missing) = FLAGGED
            .into_iter()
            .find(|name| !host_features::is_compiled(name))
        else {
            // A `full` build has nothing to mismatch against.
            return;
        };

        let config = Config::default();
        let error = validate_compiled_features(&config)
            .expect_err("a flag for a compiled-out vertical must fail")
            .to_string();
        assert!(error.contains(missing), "{error}");
        assert!(error.contains("--features"), "{error}");
    }
}
