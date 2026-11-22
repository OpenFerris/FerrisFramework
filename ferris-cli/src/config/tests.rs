use super::*;
use ferris_core::StorageBackend;
use std::path::PathBuf;

#[test]
fn test_default_config() {
    let config = CliConfig::default();
    assert_eq!(config.project.name, "ferris-app");
    assert_eq!(config.database.r#type, "postgres");
    assert_eq!(config.server.port, 8000);
}

#[test]
fn test_partial_config_merges_defaults() {
    let partial = r#"
[database]
type = "sqlite"
host = "localhost"
port = 5432
name = "app"
"#;
    let mut merged = toml::Value::try_from(CliConfig::default()).unwrap();
    merge_toml(&mut merged, toml::from_str::<toml::Value>(partial).unwrap());
    let config: CliConfig = merged.try_into().unwrap();
    assert_eq!(config.database.r#type, "sqlite");
    assert_eq!(config.database.ssl_mode, "prefer");
    assert_eq!(config.database.max_connections, 20);
    assert_eq!(config.project.name, "ferris-app");
    assert!(config.features.realtime);
    assert_eq!(
        config.apps.wasm_dir,
        PathBuf::from("./plugins"),
        "an absent [apps] section still points at `ferris build`'s output"
    );
}

#[test]
fn every_section_round_trips_through_toml() {
    let mut original = CliConfig::default();
    original.server.body_limit_mb = 96;
    original.files.backend = StorageBackend::S3;
    original.files.local_path = "/srv/blobs".to_string();
    original.files.max_file_size_mb = 123;
    original.files.allowed_extensions = vec!["pdf".to_string()];
    original.files.image_quality = 70;
    original.email.queue_concurrency = 7;
    original.email.retry_attempts = 4;
    original.search.enabled = false;
    original.search.index_prefix = "acme".to_string();
    original.realtime.port = 9101;
    original.jobs.worker_concurrency = 3;
    original.jobs.max_retries = 5;
    original.scheduler.timezone = "Europe/Madrid".to_string();
    original.scheduler.max_concurrent_jobs = 42;
    original.features.realtime = false;
    original.features.report = false;

    let text = toml::to_string(&original).expect("an all-defaults config serializes");
    let parsed: CliConfig = toml::from_str(&text).expect("its own output parses");

    assert_eq!(parsed.server.body_limit_mb, 96);
    assert_eq!(parsed.files.backend, StorageBackend::S3);
    assert_eq!(parsed.files.local_path, "/srv/blobs");
    assert_eq!(parsed.files.max_file_size_mb, 123);
    assert_eq!(parsed.files.allowed_extensions, vec!["pdf".to_string()]);
    assert_eq!(parsed.files.image_quality, 70);
    assert_eq!(parsed.email.queue_concurrency, 7);
    assert_eq!(parsed.email.retry_attempts, 4);
    assert!(!parsed.search.enabled);
    assert_eq!(parsed.search.index_prefix, "acme");
    assert_eq!(parsed.realtime.port, 9101);
    assert_eq!(parsed.jobs.worker_concurrency, 3);
    assert_eq!(parsed.jobs.max_retries, 5);
    assert_eq!(parsed.scheduler.timezone, "Europe/Madrid");
    assert_eq!(parsed.scheduler.max_concurrent_jobs, 42);
    assert!(!parsed.features.realtime);
    assert!(!parsed.features.report);
}

/// A config file written before `[email]`/`[search]` existed (or with only
/// the keys the old reduced sections had) must merge onto the defaults, not
/// fail to deserialize.
#[test]
fn a_partial_file_keeps_the_defaults_for_missing_keys() {
    let partial = r#"
[server]
body_limit_mb = 64

[features]
realtime = false

[jobs]
worker_concurrency = 4
"#;
    let mut merged = toml::Value::try_from(CliConfig::default()).unwrap();
    merge_toml(&mut merged, toml::from_str::<toml::Value>(partial).unwrap());
    let config: CliConfig = merged.try_into().expect("partial sections merge");

    assert_eq!(config.server.body_limit_mb, 64);
    assert!(!config.features.realtime);
    assert!(config.features.email, "an absent feature stays enabled");
    assert!(
        config.features.report,
        "an absent `report` flag must not disable the route"
    );
    assert_eq!(config.jobs.worker_concurrency, 4);
    assert_eq!(config.jobs.default_queue, "default");
    assert_eq!(config.scheduler.max_concurrent_jobs, 100);
    assert_eq!(config.realtime.port, 8001);
    assert_eq!(config.email.queue_concurrency, 10);
    assert!(config.search.enabled);
    assert_eq!(config.files.backend, StorageBackend::Local);
}

#[test]
fn the_cli_files_section_maps_to_the_core_section() {
    let core = FilesConfig::default().to_core();
    let expected = ferris_core::FilesConfig::default();
    assert_eq!(core.backend, expected.backend);
    assert_eq!(core.local_path, expected.local_path);
    assert_eq!(core.max_file_size_mb, expected.max_file_size_mb);
    assert_eq!(core.allowed_extensions, expected.allowed_extensions);
    assert_eq!(core.image_optimization, expected.image_optimization);
    assert_eq!(core.image_quality, expected.image_quality);
    assert!(core.cdn_url.is_none());
    assert!(core.s3.is_none());
}

/// `[apps]` used to exist only in `ferris-core` and nothing read it: the CLI
/// replaced it with `Default::default()` and the server never loaded a
/// plugin. These two pin the section on the CLI side.
#[test]
fn apps_default_to_the_build_output_directory() {
    let config = CliConfig::default();
    assert_eq!(config.apps.wasm_dir, PathBuf::from("./plugins"));
    assert!(
        config.apps.enabled.is_empty(),
        "empty means every component"
    );
}

#[test]
fn an_apps_section_is_merged_over_the_defaults() {
    let partial = r#"
[apps]
wasm_dir = "wasm"
enabled = ["my-app"]
"#;
    let mut merged = toml::Value::try_from(CliConfig::default()).unwrap();
    merge_toml(&mut merged, toml::from_str::<toml::Value>(partial).unwrap());
    let config: CliConfig = merged.try_into().unwrap();

    assert_eq!(config.apps.wasm_dir, PathBuf::from("wasm"));
    assert_eq!(config.apps.enabled, vec!["my-app".to_string()]);
}

#[test]
fn jwt_secret_has_no_usable_default() {
    let config = CliConfig::default();
    assert_eq!(config.auth.jwt_secret, None);
    assert!(
        config.require_jwt_secret().is_err(),
        "a missing JWT secret must fail closed, never fall back to a placeholder"
    );
}

#[test]
fn rejects_placeholder_jwt_secret() {
    let mut bad = CliConfig::default();
    bad.auth.jwt_secret = Some("change-me-in-production".to_string());
    assert!(bad.reject_insecure_jwt_secret().is_err());

    let mut good = CliConfig::default();
    good.auth.jwt_secret = Some("a-very-long-random-secret-0123456789".to_string());
    assert!(good.reject_insecure_jwt_secret().is_ok());
    assert_eq!(
        good.require_jwt_secret().unwrap(),
        "a-very-long-random-secret-0123456789"
    );
}

#[test]
fn parses_postgres_url_into_structured_config() {
    let mut db = DatabaseConfig {
        r#type: "sqlite".into(),
        host: "x".into(),
        port: 0,
        name: "x".into(),
        user: "x".into(),
        password: "x".into(),
        max_connections: 20,
        ssl_mode: "prefer".into(),
    };
    assert!(super::load::apply_database_url(
        "postgres://alice:s3cret@db.internal:6432/app?sslmode=require",
        &mut db,
    ));
    assert_eq!(db.r#type, "postgres");
    assert_eq!(db.host, "db.internal");
    assert_eq!(db.port, 6432);
    assert_eq!(db.user, "alice");
    assert_eq!(db.password, "s3cret");
    assert_eq!(db.name, "app");
}

/// N3: the derived `Debug` dumped `database.password` and `auth.jwt_secret`
/// in plaintext — a panic path or a stray `{:?}` leaked credentials while
/// `ferris config show` redacted them at display time. `Debug` now redacts
/// at the type level, so there is no formatting call that can leak.
#[test]
fn debug_redacts_secrets_at_the_type_level() {
    let mut config = CliConfig::default();
    config.database.password = "db-p4ssw0rd".to_string();
    config.database.user = "bob".to_string();
    config.auth.jwt_secret = Some("s3cr3t-jwt-zzz".to_string());
    config.redis.url = "redis://user:rd-p4ss@cache.internal:6379".to_string();
    config.secrets.key = Some("ab".repeat(32));
    config.custom.insert(
        "api_key".to_string(),
        serde_json::Value::String("custom-key-123".to_string()),
    );

    let dump = format!("{config:?}");
    assert!(dump.contains("<redacted>"), "{dump}");
    assert!(
        !dump.contains("db-p4ssw0rd"),
        "database password leaked: {dump}"
    );
    assert!(
        !dump.contains("s3cr3t-jwt-zzz"),
        "JWT secret leaked: {dump}"
    );
    assert!(!dump.contains("rd-p4ss"), "redis password leaked: {dump}");
    assert!(
        !dump.contains("custom-key-123"),
        "custom-map secret leaked: {dump}"
    );
    assert!(
        !dump.contains(&"ab".repeat(32)),
        "secret-field key leaked: {dump}"
    );
    // Non-secret surroundings survive for debugging.
    assert!(dump.contains("bob"), "{dump}");
    assert!(dump.contains("cache.internal"), "{dump}");
}

#[test]
fn redact_url_keeps_non_secret_parts() {
    assert_eq!(
        redact_url("redis://user:pass@cache.internal:6379"),
        "redis://user:<redacted>@cache.internal:6379"
    );
    assert_eq!(
        redact_url("redis://:pass@cache.internal:6379"),
        "redis://:<redacted>@cache.internal:6379"
    );
    assert_eq!(
        redact_url("redis://cache.internal:6379"),
        "redis://cache.internal:6379"
    );
}

#[test]
fn rejects_unsupported_database_url_scheme() {
    let mut db = DatabaseConfig {
        r#type: "sqlite".into(),
        host: "x".into(),
        port: 0,
        name: "x".into(),
        user: "x".into(),
        password: "x".into(),
        max_connections: 20,
        ssl_mode: "prefer".into(),
    };
    assert!(!super::load::apply_database_url(
        "sqlite:///tmp/app.db",
        &mut db
    ));
    // Nothing was mutated.
    assert_eq!(db.r#type, "sqlite");
}
