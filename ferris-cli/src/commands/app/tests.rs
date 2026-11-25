use super::*;
use ferris_core::site::SiteConfig;
use std::path::PathBuf;

fn config_file(contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "ferris-app-config-{}-{}.toml",
        std::process::id(),
        contents.len()
    ));
    fs::write(&path, contents).expect("config");
    path
}

/// A scratch sites+apps pair, so a test never touches the checkout.
fn scratch(label: &str) -> (PathBuf, SiteManager) {
    let root = std::env::temp_dir().join(format!(
        "ferris-app-catalogue-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("the scratch root is writable");
    let manager = SiteManager::new(root.join("sites"), root.join("apps"));
    (root, manager)
}

/// A package as the contract describes it: manifest, component and world.
fn package(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    fs::create_dir_all(dir.join("world/customizations")).expect("world");
    fs::write(dir.join(format!("{name}.wasm")), b"\0asm").expect("component");
    fs::write(
        dir.join("plugin.toml"),
        format!("name = \"{name}\"\nversion = \"1.2.3\"\n"),
    )
    .expect("manifest");
    dir
}

fn requested() -> GrantSet {
    GrantSet::new(
        vec!["process".to_string(), "http-unrestricted".to_string()],
        vec!["api.example.com".to_string()],
        vec!["bin/convert".to_string()],
    )
}

fn options<'a>(yes: bool, grant: &'a [String]) -> InstallOptions<'a> {
    InstallOptions {
        trust: None,
        allow_unsigned: false,
        yes,
        grant,
    }
}

/// Without `--yes`/`--grant` the install stops before copying anything: there
/// is no interactive prompt a non-TTY invocation could hang on.
#[test]
fn an_unsigned_package_is_refused_before_it_is_copied() {
    let (root, catalogue) = scratch("unsigned");
    let source = package(&root, "facturacion");
    let error = install_package(&catalogue, &source, None, &options(false, &[]))
        .expect_err("unsigned needs --allow-unsigned");
    assert!(error.to_string().contains("--allow-unsigned"), "{error}");
    assert!(
        catalogue.global_apps().unwrap().is_empty(),
        "a refused package never reaches the catalogue"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn approval_needs_a_flag_and_lists_the_request() {
    let error = approve_grants("facturacion", &requested(), &options(false, &[]))
        .expect_err("needs an explicit flag");
    let text = error.to_string();
    assert!(text.contains("--yes"), "{text}");
    assert!(text.contains("process"), "{text}");
    assert!(text.contains("api.example.com"), "{text}");
    assert!(text.contains("bin/convert"), "{text}");
}

#[test]
fn yes_approves_every_requested_grant() {
    let approved =
        approve_grants("facturacion", &requested(), &options(true, &[])).expect("approved");
    assert_eq!(approved, requested());
}

#[test]
fn grant_selects_individual_items() {
    let wanted = vec![
        "process".to_string(),
        "http-unrestricted".to_string(),
        "http:api.example.com".to_string(),
        "process:bin/convert".to_string(),
    ];
    let approved =
        approve_grants("facturacion", &requested(), &options(false, &wanted)).expect("approved");
    assert_eq!(approved, requested());
}

#[test]
fn an_unknown_grant_is_refused() {
    let wanted = vec!["dns-server".to_string()];
    let error = approve_grants("facturacion", &requested(), &options(false, &wanted))
        .expect_err("not requested");
    assert!(error.to_string().contains("dns-server"), "{error}");
}

#[test]
fn approving_only_some_withholds_the_rest() {
    let wanted = vec!["process".to_string()];
    let approved =
        approve_grants("facturacion", &requested(), &options(false, &wanted)).expect("approved");
    assert!(approved.allows_capability("process"));
    assert!(!approved.allows_http_host("api.example.com"));
    assert!(!approved.allows_process_bin("bin/convert"));
}

#[test]
fn an_empty_list_is_materialised_before_disabling() {
    let dir = std::env::temp_dir().join(format!("ferris-apps-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("dir");
    fs::write(dir.join("a.wasm"), b"x").expect("a");
    fs::write(dir.join("b.wasm"), b"x").expect("b");
    let config = config_file("[apps]\nenabled = []\n");
    set_enabled(&dir, &config, &[], "a", false).expect("disable");
    let text = fs::read_to_string(&config).expect("read");
    assert!(text.contains("\"b\""), "{text}");
    assert!(!text.contains("\"a\""), "{text}");
    assert_eq!(world(&dir, "a"), dir.join("a"));
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_file(&config);
}

#[test]
fn enabling_adds_a_name_to_a_non_empty_list() {
    let config = config_file("[apps]\nenabled = [\"a\"]\n");
    add_enabled(&config, "b").expect("add");
    let text = fs::read_to_string(&config).expect("read");
    assert!(text.contains("\"a\"") && text.contains("\"b\""), "{text}");
    let _ = fs::remove_file(&config);
}

/// A package directory installs into the catalogue and a site can then take
/// a copy without the template being written by the site.
#[test]
fn a_package_lands_in_the_catalogue_and_a_site_gets_an_instance() {
    let (root, catalogue) = scratch("package");
    let source = package(&root, "facturacion");

    let manifest = catalogue.install_global_app(&source).expect("install");
    assert_eq!(manifest.name, "facturacion");
    assert_eq!(manifest.version, "1.2.3");
    assert_eq!(catalogue.global_apps().unwrap(), vec!["facturacion"]);
    assert!(
        catalogue
            .global_app_dir("facturacion")
            .unwrap()
            .join("facturacion.wasm")
            .is_file()
    );

    let mut site = SiteConfig::new("erp");
    site.host_name = "erp.example.com".to_string();
    catalogue.create(site).expect("site");
    catalogue
        .install_app("erp", "facturacion", false)
        .expect("install into the site");
    let instance = catalogue.instance_world("erp", "facturacion").unwrap();
    assert!(instance.join("config.toml").is_file());
    assert!(
        instance.join("secrets").is_dir(),
        "the instance world exists"
    );
    assert_eq!(
        catalogue.installed_apps("erp").unwrap(),
        vec!["facturacion".to_string()]
    );

    let _ = fs::remove_dir_all(&root);
}

/// A bare component is wrapped in a minimal package, installed, and the
/// temporary package is gone afterwards.
#[test]
fn a_bare_component_is_wrapped_installed_and_the_package_removed() {
    let (root, catalogue) = scratch("component");
    let source = root.join("demo.wasm");
    fs::write(&source, b"\0asm").expect("component");

    let package = root.join("wrapped");
    build_package(&package, &source, "demo").expect("package");
    assert!(package.join("plugin.toml").is_file());
    assert!(package.join("demo.wasm").is_file());
    for dir in WORLD_DIRS {
        assert!(package.join("world").join(dir).is_dir(), "{dir}");
    }
    let manifest = catalogue.install_global_app(&package).expect("install");
    assert_eq!(manifest.name, "demo");
    assert_eq!(manifest.version, "0.1.0");
    assert_eq!(catalogue.global_apps().unwrap(), vec!["demo"]);
    fs::remove_dir_all(&package).expect("cleanup");
    assert!(!package.exists());

    let _ = fs::remove_dir_all(&root);
}

/// The catalogue refuses to drop a template a site still lists, and names
/// the site so the operator knows what to uninstall first.
#[test]
fn a_catalogue_app_installed_in_a_site_is_not_removed() {
    let (root, catalogue) = scratch("in-use");
    let source = package(&root, "facturacion");
    catalogue.install_global_app(&source).expect("install");
    let mut site = SiteConfig::new("erp");
    site.host_name = "erp.example.com".to_string();
    catalogue.create(site).expect("site");
    catalogue
        .install_app("erp", "facturacion", false)
        .expect("instance");

    let err = remove_from_catalogue(&catalogue, "facturacion", false)
        .expect_err("an installed app must not be removed");
    assert!(err.to_string().contains("erp"), "{err}");
    assert!(err.to_string().contains("uninstall-app"), "{err}");
    assert!(catalogue.global_app_dir("facturacion").unwrap().is_dir());

    catalogue
        .uninstall_app("erp", "facturacion", false)
        .expect("uninstall");
    remove_from_catalogue(&catalogue, "facturacion", false).expect("now it can go");
    assert!(!catalogue.global_app_dir("facturacion").unwrap().exists());

    let _ = fs::remove_dir_all(&root);
}

/// The two-scope commands only take over when `[apps] wasm_dir` is the
/// catalogue: a deployment on the flat `plugins/` contract keeps working.
#[test]
fn the_catalogue_mode_is_opt_in() {
    let project = Path::new("/srv/app");
    let catalogue = SiteManager::new(project.join("sites"), project.join("apps"));
    let legacy = ferris_core::AppsConfig::default();
    assert_ne!(plugins_dir(project, &legacy), catalogue.apps_root());
    assert_eq!(plugins_dir(project, &legacy), project.join("plugins"));

    let catalogue_config = ferris_core::AppsConfig {
        wasm_dir: PathBuf::from("apps"),
        ..ferris_core::AppsConfig::default()
    };
    assert_eq!(
        plugins_dir(project, &catalogue_config),
        catalogue.apps_root()
    );
}

/// A site name that is not a path segment must be refused by `app list
/// --site` before any directory is read.
#[test]
fn a_bad_site_name_is_refused() {
    let (root, catalogue) = scratch("bad-site");
    assert!(list_site(&catalogue, "../etc").is_err());
    assert!(!ferris_core::site::is_valid_site_name("../etc"));
    let _ = fs::remove_dir_all(&root);
}
