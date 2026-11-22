use super::checks::{check_redis, redis_required};
use super::features::check_features;
use super::migrations::{Migrations, check_migrations, inspect_migrations};
use crate::config::CliConfig;

/// Scratch directory per test, since `cargo test` runs them in threads.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ferris-doctor-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_missing_migrations_dir_is_not_a_failed_check() {
    let dir = scratch("no-migrations");
    assert_eq!(
        inspect_migrations(&dir.join("migrations")),
        Migrations::Missing
    );

    let check = check_migrations(&dir);
    assert!(check.passed, "{}", check.message);
    assert!(
        check.message.contains("generated from doctypes"),
        "{}",
        check.message
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_subdirectory_is_not_a_migration() {
    let dir = scratch("subdirectory");
    let migrations = dir.join("migrations");
    std::fs::create_dir_all(migrations.join("0001_init")).unwrap();
    std::fs::write(migrations.join("0002_add_status.sql"), "select 1;").unwrap();

    // `entries.count()` said "2 migration file(s)" here, one of which was a
    // directory that held no migration.
    assert_eq!(
        inspect_migrations(&migrations),
        Migrations::Found {
            files: 1,
            unreadable: vec![]
        }
    );
    let check = check_migrations(&dir);
    assert!(
        check.message.contains("1 migration file(s)"),
        "{}",
        check.message
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_migrations_path_that_is_a_file_is_not_an_empty_dir() {
    let dir = scratch("migrations-is-a-file");
    // `read_dir` on a file fails, and the old code reported that as the
    // reassuring "no migrations/ dir".
    std::fs::write(dir.join("migrations"), "not a directory").unwrap();

    match inspect_migrations(&dir.join("migrations")) {
        Migrations::Unreadable(why) => assert!(why.contains("cannot read"), "{why}"),
        other => panic!("a file where migrations/ should be is unreadable, got {other:?}"),
    }

    let check = check_migrations(&dir);
    assert!(
        !check.passed,
        "a directory nobody could look in is not fine"
    );
    assert!(
        !check.critical,
        "but it does not stop the project from building"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A scaffolded config turns realtime on, so Redis is needed even when the
/// URL is the untouched default. The old condition made every fresh project
/// fail this check while claiming the opposite in its message.
#[test]
fn a_scaffolded_project_does_need_redis() {
    let config = CliConfig::default();
    assert!(config.realtime.enabled, "realtime is on by default");
    assert!(
        redis_required(&config),
        "with realtime on, Redis is not optional"
    );
}

#[test]
fn redis_is_optional_once_realtime_is_off() {
    let mut config = CliConfig::default();
    assert!(redis_required(&config), "realtime is on by default");

    // The job queue no longer needs Redis (R-FW-1a): it runs on the durable
    // `jobs` table alone, so turning realtime off makes Redis optional.
    config.realtime.enabled = false;
    assert!(
        !redis_required(&config),
        "only realtime hard-requires Redis"
    );

    // And then the check says so instead of pretending it checked something.
    let check = check_redis(&config);
    assert!(check.passed);
    assert!(
        check.message.contains("realtime is disabled"),
        "{}",
        check.message
    );
}

#[test]
fn an_enabled_feature_with_no_url_is_reported_not_passed() {
    let mut config = CliConfig::default();
    config.redis.url = String::new();
    let check = check_redis(&config);
    assert!(!check.passed, "an empty url is not a working Redis");
    assert!(
        check.message.contains("redis.url is empty"),
        "{}",
        check.message
    );
}

/// A config that asks for nothing, so the feature check answers the same way
/// whatever features the test binary was compiled with.
fn quiet_features() -> CliConfig {
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

#[test]
fn the_feature_check_always_prints_both_sides() {
    let config = quiet_features();
    let check = check_features(&config);

    assert!(check.passed, "{}", check.message);
    assert!(
        check.critical,
        "a boot-blocking condition is a critical check even when it passes"
    );
    assert!(check.message.contains("compiled:"), "{}", check.message);
    assert!(check.message.contains("configured:"), "{}", check.message);
}
