use super::*;
use std::path::PathBuf;

/// A scratch sites+apps pair, so a test never touches the checkout.
fn scratch(label: &str) -> (PathBuf, SiteManager) {
    let root = std::env::temp_dir().join(format!(
        "ferris-site-cli-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("the scratch root is writable");
    let manager = SiteManager::new(root.join("sites"), root.join("apps"));
    (root, manager)
}

fn site_cli() -> CliConfig {
    let mut cli = CliConfig::default();
    cli.database.r#type = "postgres".to_string();
    cli.database.host = "db.internal".to_string();
    cli.database.port = 6432;
    cli.database.name = "project".to_string();
    cli.database.user = "ferris".to_string();
    cli.database.password = "secret".to_string();
    cli.database.max_connections = 12;
    cli.database.ssl_mode = "require".to_string();
    cli.redis.url = "redis://redis.internal:6379".to_string();
    cli
}

/// `--site` has to move the whole process: database, files, Redis, apps and
/// the instance scope the guest mounts at `/world`.
#[test]
fn a_site_rewrites_the_database_files_redis_and_apps() {
    let mut cli = site_cli();
    let mut site = SiteConfig::new("erp.example.com");
    site.host_name = "erp.example.com".to_string();
    site.database.r#type = ferris_core::DatabaseType::MySQL;
    site.database.host = "site-db".to_string();
    site.database.port = 3307;
    site.database.name = "erp".to_string();
    site.database.user = "erp_user".to_string();
    site.database.password = "site-secret".to_string();
    site.database.max_connections = 4;
    site.database.ssl_mode = ferris_core::SslMode::VerifyFull;
    site.redis_url = "redis://site-redis:6380".to_string();
    site.files_path = PathBuf::from("/srv/sites/erp/private/files");
    site.apps = vec!["facturacion".to_string()];
    let plugins = PathBuf::from("/srv/sites/erp.example.com/plugins");

    apply_site(&mut cli, &site, &plugins);

    assert_eq!(cli.database.r#type, "mysql");
    assert_eq!(cli.database.host, "site-db");
    assert_eq!(cli.database.port, 3307);
    assert_eq!(cli.database.name, "erp");
    assert_eq!(cli.database.user, "erp_user");
    assert_eq!(cli.database.password, "site-secret");
    assert_eq!(cli.database.max_connections, 4);
    assert_eq!(cli.database.ssl_mode, "verify-full");
    assert_eq!(cli.redis.url, "redis://site-redis:6380");
    assert_eq!(cli.files.local_path, "/srv/sites/erp/private/files");
    assert_eq!(cli.apps.enabled, vec!["facturacion".to_string()]);
    assert_eq!(cli.apps.wasm_dir, plugins);
}

#[test]
fn a_disabled_site_is_refused() {
    let mut site = SiteConfig::new("erp");
    assert!(usable(&site).is_ok());
    site.enabled = false;
    let err = usable(&site).expect_err("a disabled site must not boot");
    assert!(err.to_string().contains("disabled"), "{err}");
    assert!(
        err.to_string().contains("ferris site enable erp"),
        "the error names the fix: {err}"
    );
}

#[test]
fn the_host_allow_list_keeps_the_file_entries_and_adds_the_site() {
    let path =
        std::env::temp_dir().join(format!("ferris-site-hosts-{}.toml", uuid::Uuid::new_v4()));
    std::fs::write(&path, "[server]\nallowed_hosts = [\"desk.example.com\"]\n").expect("config");
    let mut cli = site_cli();
    cli.source.config_path = path.clone();

    let mut site = SiteConfig::new("erp");
    site.host_name = "ERP.example.com:8443".to_string();
    let hosts = allowed_hosts_for(&cli, &site).expect("hosts");
    assert_eq!(hosts, vec!["desk.example.com", "ERP.example.com:8443"]);

    // The same name in another spelling is not added twice.
    let mut same = SiteConfig::new("desk");
    same.host_name = "desk.example.com".to_string();
    let hosts = allowed_hosts_for(&cli, &same).expect("hosts");
    assert_eq!(hosts, vec!["desk.example.com"]);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn no_allowed_hosts_in_the_file_still_lists_the_site() {
    let missing = std::env::temp_dir().join(format!(
        "ferris-site-no-hosts-{}.toml",
        uuid::Uuid::new_v4()
    ));
    let mut cli = site_cli();
    cli.source.config_path = missing;
    let site = SiteConfig::new("erp.example.com");
    assert_eq!(
        allowed_hosts_for(&cli, &site).expect("hosts"),
        vec!["erp.example.com".to_string()]
    );
}

/// A duplicate host is refused before the admin user is created, and the
/// error names the site that already owns it.
#[test]
fn a_duplicate_host_is_refused_before_provisioning() {
    let (root, manager) = scratch("hosts");
    let mut first = SiteConfig::new("one");
    first.host_name = "erp.example.com".to_string();
    manager.create(first).expect("the first site");

    let mut second = SiteConfig::new("two");
    second.host_name = "erp.example.com".to_string();
    let err = ensure_host_free(&manager, &second).expect_err("the host is taken");
    assert!(err.to_string().contains("site `one`"), "{err}");

    // A different host passes.
    second.host_name = "other.example.com".to_string();
    assert!(ensure_host_free(&manager, &second).is_ok());

    let _ = std::fs::remove_dir_all(&root);
}

/// `--db-name` renames the database; `--db-type` is validated, not guessed.
#[test]
fn a_site_database_takes_the_project_credentials_and_its_own_name() {
    let cli = site_cli();
    let database = database_for(&cli, "erp_dev".to_string(), None).expect("database");
    assert_eq!(database.name, "erp_dev");
    assert_eq!(database.host, "db.internal");
    assert_eq!(database.port, 6432);
    assert_eq!(database.user, "ferris");
    assert_eq!(database.password, "secret");
    assert_eq!(database.r#type, ferris_core::DatabaseType::Postgres);

    let sqlite = database_for(&cli, "erp_dev".to_string(), Some("sqlite")).expect("database");
    assert_eq!(sqlite.r#type, ferris_core::DatabaseType::SQLite);

    assert!(
        database_for(&cli, "erp_dev".to_string(), Some("postgress")).is_err(),
        "a typo must not fall back to postgres"
    );
}

#[test]
fn the_database_name_defaults_to_the_site_name() {
    // The value `SiteConfig::new` derives, which is what `site new` uses
    // when `--db-name` is absent: dots and dashes are not SQL identifiers.
    let site = SiteConfig::new("erp.example.com");
    assert_eq!(site.database.name, "erp_example_com");
    assert_eq!(site.database.user, "erp_example_com");
}

#[test]
fn a_malformed_allowed_hosts_table_is_an_error() {
    let path = std::env::temp_dir().join(format!(
        "ferris-site-bad-hosts-{}.toml",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&path, "[server]\nallowed_hosts = \"erp.example.com\"\n").expect("config");
    let err = allowed_hosts_from_file(&path).expect_err("a string is not a list");
    assert!(err.to_string().contains("array"), "{err}");
    let _ = std::fs::remove_file(&path);
}

/// A SQLite site is one directory: the database lives under its `private`
/// root, not at the bench root where two sites could collide and `site delete`
/// would leave it behind.
#[test]
fn a_relative_sqlite_database_lives_in_the_site_private_directory() {
    let (root, manager) = scratch("sqlite-private");
    let mut site = SiteConfig::new("erp");
    site.database.r#type = ferris_core::DatabaseType::SQLite;
    site.database.name = "erp.db".to_string();
    let site = manager.create(site).expect("the site is created");

    let database = site_database(&manager, &site);

    let expected = root
        .join("sites")
        .join("erp")
        .join("private")
        .join("erp.db");
    assert_eq!(database.name, expected.display().to_string());
    assert!(root.join("sites/erp/private").is_dir());
}

/// A site created before the layout kept the file at the bench root; opening it
/// moves the file (and its sidecars) once, preserving the bytes.
#[test]
fn a_bench_root_sqlite_file_is_adopted_once() {
    let (root, manager) = scratch("sqlite-adopt");
    let mut site = SiteConfig::new("erp");
    site.database.r#type = ferris_core::DatabaseType::SQLite;
    site.database.name = "erp".to_string();
    let site = manager.create(site).expect("the site is created");

    std::fs::write(root.join("erp"), b"sqlite-bytes").expect("the legacy file is written");
    std::fs::write(root.join("erp-wal"), b"wal-bytes").expect("the legacy wal is written");

    let database = site_database(&manager, &site);
    let target = root.join("sites/erp/private/erp");
    assert_eq!(database.name, target.display().to_string());
    assert_eq!(
        std::fs::read(&target).expect("the file moved"),
        b"sqlite-bytes"
    );
    assert_eq!(
        std::fs::read(root.join("sites/erp/private/erp-wal")).expect("the wal moved"),
        b"wal-bytes"
    );
    assert!(!root.join("erp").exists(), "the bench root keeps no copy");

    // Once the site has its own file, a stray legacy file is not adopted: the
    // site's copy is authoritative.
    std::fs::write(root.join("erp"), b"stale").expect("a stale file is written");
    let again = site_database(&manager, &site);
    assert_eq!(again.name, target.display().to_string());
    assert_eq!(
        std::fs::read(&target).expect("the site copy is read"),
        b"sqlite-bytes"
    );
    assert!(root.join("erp").exists(), "the stray file is left alone");
}

/// A `db_name` that is a path keeps its historical meaning (relative to the
/// bench root) instead of being silently rebased under `private/`.
#[test]
fn a_path_shaped_sqlite_name_stays_at_the_bench_root() {
    let (root, manager) = scratch("sqlite-path");
    let mut site = SiteConfig::new("erp");
    site.database.r#type = ferris_core::DatabaseType::SQLite;
    site.database.name = "dbs/erp.db".to_string();
    let site = manager.create(site).expect("the site is created");

    let database = site_database(&manager, &site);

    assert_eq!(database.name, root.join("dbs/erp.db").display().to_string());
    assert!(!root.join("sites/erp/private/dbs").exists());
}
