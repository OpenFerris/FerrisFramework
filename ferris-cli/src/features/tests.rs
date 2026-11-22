//! Tests for the compile-time feature matrix.
//!
//! Every check takes the compiled set as a parameter, so these tests inject one
//! instead of reading `compiled()`: the test binary is built with the server's
//! default features, and a test that relied on that would assert different
//! things under `--no-default-features` and quietly stop covering the minimal
//! build.

use super::*;

#[test]
fn every_name_is_a_declared_server_feature() {
    // The reports print these names verbatim and the checks compare them
    // against the set the server builds from `cfg!`; a rename on either side
    // would make a check silently pass.
    for feature in Feature::ALL {
        assert!(
            ferris_server::host_features::FEATURES.contains(&feature.name()),
            "`{}` is not in the server's feature list",
            feature.name()
        );
    }
    let unique: std::collections::BTreeSet<&str> = Feature::ALL.iter().map(|f| f.name()).collect();
    assert_eq!(unique.len(), Feature::ALL.len(), "duplicate feature name");
}

#[test]
fn the_compiled_set_comes_from_the_server() {
    let compiled = compiled();
    assert!(compiled.contains("api"));
    assert_eq!(compiled, ferris_server::host_features::compiled());
}

#[test]
fn the_report_label_is_readable() {
    assert_eq!(describe(&HostFeatures::new(["print", "api"])), "api, print");
    assert_eq!(describe(&HostFeatures::default()), "none");
}

#[test]
fn a_minimal_build_with_the_default_flags_reports_each_missing_subsystem() {
    // `FeatureFlags::default()` leaves the verticals on. A binary built
    // without them cannot honour that, and the point of the check is to say so
    // per flag instead of booting a server that answers nothing.
    let config = CliConfig::default();
    let found = mismatches(&config, &api_only());

    let features: Vec<Feature> = found.iter().map(|m| m.feature).collect();
    assert!(features.contains(&Feature::Realtime), "{found:?}");
    assert!(features.contains(&Feature::Email), "{found:?}");
    assert!(
        !features.contains(&Feature::Api),
        "the base is never missing"
    );
}

#[test]
fn a_disabled_flag_is_never_a_mismatch() {
    let config = quiet_config();
    assert!(mismatches(&config, &api_only()).is_empty());
    validate(&config, &api_only()).expect("nothing is asked for, nothing is missing");
}

#[test]
fn search_and_webhooks_are_not_checked_against_cargo_features() {
    // They are runtime switches over code that ships in the always-linked API
    // and integrations paths; the server's validator draws the same line.
    let mut config = quiet_config();
    config.features.search = true;
    config.features.webhooks = true;
    assert!(mismatches(&config, &api_only()).is_empty());
}

#[test]
fn validate_returns_the_configuration_error_variant() {
    let mut config = quiet_config();
    config.features.realtime = true;
    let err = validate(&config, &api_only()).expect_err("a mismatch must fail");
    match err {
        Error::Config(ConfigError::Invalid { key, msg }) => {
            assert_eq!(key, "features.realtime");
            assert!(msg.contains("rebuild ferris"), "{msg}");
        }
        other => panic!("expected the config taxonomy, got {other:?}"),
    }
}

#[test]
fn configured_summary_lists_every_switch() {
    let mut config = CliConfig::default();
    config.features.realtime = false;
    config.features.print = true;
    let summary = configured_summary(&config.features);

    for name in [
        "realtime",
        "jobs",
        "scheduler",
        "email",
        "files",
        "print",
        "report",
        "search",
        "webhooks",
    ] {
        assert!(summary.contains(name), "`{name}` missing from {summary}");
    }
    assert!(summary.contains("realtime=off"), "{summary}");
    assert!(summary.contains("print=on"), "{summary}");
    assert!(
        !summary.contains("features."),
        "the summary is the switch names, not config keys: {summary}"
    );
}

fn api_only() -> HostFeatures {
    HostFeatures::new(["api"])
}

/// A config that asks for nothing, so a test can pass `api_only()` without the
/// default flags producing noise.
fn quiet_config() -> CliConfig {
    let mut config = CliConfig::default();
    config.features.realtime = false;
    config.features.jobs = false;
    config.features.scheduler = false;
    config.features.email = false;
    config.features.files = false;
    config.features.print = false;
    config.features.report = false;
    config.features.webhooks = false;
    config
}
