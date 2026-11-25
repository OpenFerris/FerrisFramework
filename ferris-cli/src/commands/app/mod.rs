//! `ferris app`: the lifecycle of the WASM apps an install publishes.
//!
//! Two scopes, one command (see `auditoria/parches/SITES.md`). An app is
//! installed **globally** once, into `apps/<app>/` (`plugin.toml`, `<app>.wasm`
//! and a `world/` template); a site receives a **copy** of that template plus its
//! own `config.toml` and data through `ferris site install-app`. The wasm is
//! referenced, never duplicated, and upgrading the catalogue never overwrites a
//! site's data.
//!
//! The command keeps the older single-scope layout working: when `[apps] wasm_dir`
//! does not name the global catalogue (`<project>/apps`), the flat
//! `plugins/<app>.wasm` contract of `ferris build` is what the process publishes,
//! and `install`/`remove`/`list` operate on it exactly as before. Deployments that
//! did not opt into `apps/` must not change behaviour because the catalogue
//! exists.
//!
//! `disable` writes an explicit allow-list. An empty list means "every
//! component", so disabling one app has to materialise the names of the others
//! first; that is the only place the legacy path is more than a file move.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use ferris_core::plugins::{WORLD_DIRS, ensure_world_at, is_valid_app_name};
use ferris_core::site::{AppManifest, SiteManager};
use ferris_wasm::trust::{self, GrantSet, TrustStore};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use super::AppArgs;
use super::AppCommand;
use super::plugin_boot::{discover, plugins_dir};
use crate::config::CliConfig;

/// Dispatch `ferris app`.
///
/// # Errors
///
/// Whatever the subcommand fails on: an unreadable package, an invalid name, a
/// catalogue app that a site still installs, or an I/O failure under
/// `apps/`/`[apps] wasm_dir`.
pub fn run(args: AppArgs, config: &CliConfig) -> Result<()> {
    let dir = plugins_dir(&config.source.bench_root, &config.apps);
    // The catalogue is the two-scope layout; anything else is an install that
    // still publishes the flat `plugins/` directory.
    let catalogue = super::site::manager(config);
    let global = dir == catalogue.apps_root();
    match args.command {
        AppCommand::List { site } => {
            if global {
                list_catalogue(&catalogue, site.as_deref())
            } else {
                list(&dir, &config.apps.enabled)?;
                if let Some(site) = site.as_deref() {
                    println!();
                    list_site(&catalogue, site)?;
                }
                Ok(())
            }
        }
        AppCommand::Install {
            path,
            name,
            trust,
            allow_unsigned,
            yes,
            grant,
        } => {
            let options = InstallOptions {
                trust: trust.as_deref(),
                allow_unsigned,
                yes,
                grant: &grant,
            };
            if global {
                install_package(&catalogue, &path, name.as_deref(), &options)
            } else {
                install(
                    &dir,
                    &config.source.config_path,
                    &config.apps.enabled,
                    &path,
                    name.as_deref(),
                    &options,
                )
            }
        }
        AppCommand::Sign { app, key } => sign_package(&app, &key),
        AppCommand::Verify { app, trust } => verify_package(&app, &trust),
        AppCommand::Remove { name, purge } => {
            if global {
                remove_from_catalogue(&catalogue, &name, purge)
            } else {
                remove(
                    &dir,
                    &config.source.config_path,
                    &config.apps.enabled,
                    &name,
                    purge,
                )
            }
        }
        AppCommand::Enable { name } => set_enabled(
            &dir,
            &config.source.config_path,
            &config.apps.enabled,
            &name,
            true,
        ),
        AppCommand::Disable { name } => set_enabled(
            &dir,
            &config.source.config_path,
            &config.apps.enabled,
            &name,
            false,
        ),
    }
}

// ---------------------------------------------------------------------------
// Signing, verification and operator approval
// ---------------------------------------------------------------------------

/// The install-time trust inputs, shared by the catalogue and flat layouts.
struct InstallOptions<'a> {
    /// Trust store for a signed package.
    trust: Option<&'a Path>,
    /// Explicit escape hatch for a package with no `app.sig`.
    allow_unsigned: bool,
    /// Approve every grant the manifest requests.
    yes: bool,
    /// Approve individual grants (repeatable).
    grant: &'a [String],
}

/// Verify a package before anything is copied.
///
/// A signed package needs `--trust`; an unsigned one needs `--allow-unsigned`.
/// Both refusals happen here, so the caller's copy path is never reached with a
/// package that failed trust. Signing binds `plugin.toml`'s trust fields and
/// the component hash, so [`trust::verify_package`] rebuilds the manifest from
/// disk before checking the signature.
fn verify_source(
    source: &Path,
    options: &InstallOptions<'_>,
) -> Result<Option<trust::VerifiedManifest>> {
    let signature = source.join(trust::SIGNATURE_FILE);
    if !signature.is_file() {
        if options.allow_unsigned {
            println!(
                "warning: {} is unsigned; installing it unattributable because --allow-unsigned was given",
                source.display()
            );
            return Ok(None);
        }
        anyhow::bail!(
            "{} has no {}: a package must be signed by a trusted publisher. \
             Pass --trust <trust.toml> for a signed package, or --allow-unsigned to \
             accept an unattributable one",
            source.display(),
            trust::SIGNATURE_FILE
        );
    }
    let trust_path = options.trust.ok_or_else(|| {
        anyhow::anyhow!(
            "{} is signed; pass --trust <trust.toml> so the signature can be verified",
            source.display()
        )
    })?;
    let store = TrustStore::from_path(trust_path)
        .with_context(|| format!("cannot read the trust store {}", trust_path.display()))?;
    let verified = trust::verify_package(source, &store)
        .with_context(|| format!("cannot verify {}", source.display()))?;
    println!(
        "signed app: {} {}",
        verified.manifest.name, verified.manifest.version
    );
    println!("  publisher: {}", publisher_label(&verified.publisher));
    println!("  key:       {}", verified.key_id);
    describe_grants("requested grants", &verified.grants);
    Ok(Some(verified))
}

/// Turn the manifest's request into the operator's approval.
///
/// `--yes` approves everything. `--grant` approves one item at a time; a grant
/// that matches nothing is an error, and whatever is left unapproved is printed
/// so the operator sees the difference. With neither flag, the install stops
/// before copying and lists the request. There is no blocking prompt, so a
/// non-TTY invocation fails loudly instead of hanging.
fn approve_grants(
    app: &str,
    requested: &GrantSet,
    options: &InstallOptions<'_>,
) -> Result<GrantSet> {
    if requested.is_empty() {
        return Ok(GrantSet::default());
    }
    if options.yes {
        return Ok(requested.clone());
    }
    if options.grant.is_empty() {
        anyhow::bail!(
            "{} requests grants the operator must approve:\n{}\n\
             Re-run with --yes to approve all of them, or --grant <GRANT> \
             (repeatable) to approve only some",
            app,
            grant_lines(requested)
        );
    }

    let mut approved = GrantSet::default();
    for raw in options.grant {
        let item = raw.trim();
        let host = item
            .strip_prefix("http:")
            .or_else(|| item.strip_prefix("http-host:"))
            .unwrap_or(item);
        let bin = item.strip_prefix("process:").unwrap_or(item);
        if requested.allows_capability(item) {
            approved.capabilities.push(item.to_string());
        } else if requested.allows_http_host(host) {
            approved.http_hosts.push(host.to_string());
        } else if requested.allows_process_bin(bin) {
            approved.process_bins.push(bin.to_string());
        } else {
            anyhow::bail!(
                "`{item}` is not a grant {} requests. Requested:\n{}",
                app,
                grant_lines(requested)
            );
        }
    }
    approved.normalize();
    if approved != *requested {
        describe_grants("withheld grants", &requested_withheld(requested, &approved));
    }
    Ok(approved)
}

fn requested_withheld(requested: &GrantSet, approved: &GrantSet) -> GrantSet {
    GrantSet::new(
        requested
            .capabilities
            .iter()
            .filter(|entry| !approved.capabilities.contains(*entry))
            .cloned()
            .collect(),
        requested
            .http_hosts
            .iter()
            .filter(|entry| !approved.http_hosts.contains(*entry))
            .cloned()
            .collect(),
        requested
            .process_bins
            .iter()
            .filter(|entry| !approved.process_bins.contains(*entry))
            .cloned()
            .collect(),
    )
}

fn publisher_label(publisher: &str) -> &str {
    if publisher.trim().is_empty() {
        "(unspecified)"
    } else {
        publisher
    }
}

fn describe_grants(label: &str, grants: &GrantSet) {
    println!("  {label}:");
    println!("    capabilities: {}", join_or_none(&grants.capabilities));
    println!("    http hosts:   {}", join_or_none(&grants.http_hosts));
    println!("    process bins: {}", join_or_none(&grants.process_bins));
}

fn grant_lines(grants: &GrantSet) -> String {
    format!(
        "  capabilities: {}\n  http hosts:   {}\n  process bins: {}",
        join_or_none(&grants.capabilities),
        join_or_none(&grants.http_hosts),
        join_or_none(&grants.process_bins)
    )
}

fn join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "(none)".to_string()
    } else {
        values.join(", ")
    }
}

/// Persist the approval next to the installed app.
///
/// Catalogue: `apps/<app>/grants.toml`, and a copy at
/// `apps/<app>/world/grants.toml` so `ferris site install-app` carries it into
/// every instance world the host actually reads (`plugins/<app>/grants.toml` in
/// the flat layout). [`trust::GrantSet::load`] reads it back; a missing file
/// means "nothing approved".
fn write_grants(app_dir: &Path, grants: &GrantSet) -> Result<()> {
    let path = app_dir.join(trust::GRANTS_FILE);
    grants
        .save(&path)
        .with_context(|| format!("cannot write the approved grants to {}", path.display()))?;
    println!("  approved grants: {}", path.display());
    Ok(())
}

/// `ferris app sign`: write the key id into `plugin.toml`, hash the component
/// and write `app.sig`.
fn sign_package(app: &Path, key_path: &Path) -> Result<()> {
    if !app.is_dir() {
        anyhow::bail!("{} is not a package directory", app.display());
    }
    let key = trust::SigningKey::from_pkcs8_file(key_path).with_context(|| {
        format!(
            "cannot read {} as a PKCS#8 Ed25519 private key",
            key_path.display()
        )
    })?;
    let manifest_path = app.join(trust::PLUGIN_MANIFEST);
    let text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("cannot read {}", manifest_path.display()))?;
    let mut doc: DocumentMut = text.parse().map_err(|error| {
        anyhow::anyhow!("{} is not valid TOML: {error}", manifest_path.display())
    })?;
    let key_id = key.key_id();
    doc["key_id"] = toml_edit::value(key_id.as_str());
    fs::write(&manifest_path, doc.to_string())
        .with_context(|| format!("cannot write {}", manifest_path.display()))?;

    let manifest = trust::AppManifest::from_package(app)
        .with_context(|| format!("cannot describe the package {}", app.display()))?;
    let signature = trust::sign_manifest(&manifest, &key)
        .with_context(|| format!("cannot sign {}", manifest_path.display()))?;
    let signature_path = app.join(trust::SIGNATURE_FILE);
    fs::write(&signature_path, format!("{signature}\n"))
        .with_context(|| format!("cannot write {}", signature_path.display()))?;

    println!("signed {} {}", manifest.name, manifest.version);
    println!("  publisher: {}", publisher_label(&manifest.publisher));
    println!("  key:       {key_id}");
    println!("  public key: {}", key.public_key_base64());
    println!("  wrote:     {}", signature_path.display());
    describe_grants("signed grants", &manifest.grants());
    Ok(())
}

/// `ferris app verify`: check an installed or packaged app against a trust
/// store and print the signed description.
fn verify_package(app: &Path, trust_path: &Path) -> Result<()> {
    let store = TrustStore::from_path(trust_path)
        .with_context(|| format!("cannot read the trust store {}", trust_path.display()))?;
    let verified = trust::verify_package(app, &store)
        .with_context(|| format!("cannot verify {}", app.display()))?;
    println!(
        "verified {} {}",
        verified.manifest.name, verified.manifest.version
    );
    println!("  publisher: {}", publisher_label(&verified.publisher));
    println!("  key:       {}", verified.key_id);
    describe_grants("signed grants", &verified.grants);
    Ok(())
}

// ---------------------------------------------------------------------------
// Global catalogue (`apps/<app>/`)
// ---------------------------------------------------------------------------

/// Install a package directory, or a bare component as a one-app package.
///
/// A directory is validated by [`AppManifest::read`]: it must carry
/// `plugin.toml`, `<app>.wasm` and `world/`. A `.wasm` has no manifest, so a
/// throw-away package is assembled in the temporary directory — manifest, the
/// component and an empty world template — installed from there and deleted.
/// Nothing of that package survives: the catalogue only keeps what it copies.
fn install_package(
    catalogue: &SiteManager,
    source: &Path,
    name: Option<&str>,
    options: &InstallOptions<'_>,
) -> Result<()> {
    let known = catalogue.global_apps()?;
    let manifest = if source.is_dir() {
        // Trust is checked before any copy: a failed signature or an
        // unapproved grant leaves the catalogue byte-for-byte untouched.
        let verified = verify_source(source, options)?;
        // Validate before publishing: a component that cannot instantiate
        // against this binary must not replace a working one and take every
        // site down on its next boot.
        let package = AppManifest::read(source)?;
        let requested = verified
            .as_ref()
            .map_or_else(GrantSet::default, |verified| verified.grants.clone());
        let approved = approve_grants(&package.name, &requested, options)?;
        let described = validate_component(&package.wasm, &package.name)?;
        // Declared locales and shipped catalogues must agree before the
        // catalogue (and every site that copies it) is touched.
        let declared = described.get("locales").and_then(|value| {
            value.as_array().map(|tags| {
                tags.iter()
                    .filter_map(|tag| tag.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
        });
        let tags = super::app_catalogues::validate(source, declared.as_deref())?;
        if !tags.is_empty() {
            tracing::info!("Package ships catalogues for: {}", tags.join(", "));
        }
        let manifest = catalogue.install_global_app(source)?;
        let destination = catalogue.global_app_dir(&manifest.name)?;
        if verified.is_some() {
            fs::copy(
                source.join(trust::SIGNATURE_FILE),
                destination.join(trust::SIGNATURE_FILE),
            )
            .with_context(|| format!("cannot copy the signature into {}", destination.display()))?;
        }
        write_grants(&destination, &approved)?;
        write_grants(&destination.join("world"), &approved)?;
        manifest
    } else {
        if !options.allow_unsigned {
            anyhow::bail!(
                "{} is a bare component with no signed manifest. Install a package \
                 directory (with plugin.toml, <app>.wasm and app.sig), or pass \
                 --allow-unsigned to accept an unattributable component",
                source.display()
            );
        }
        let manifest = install_component(catalogue, source, name)?;
        let destination = catalogue.global_app_dir(&manifest.name)?;
        // No signature means no trusted request: approve nothing. Runtime
        // capabilities still come from the WIT declaration and the host's
        // grant computation; this file is the operator's additional approval.
        write_grants(&destination, &GrantSet::default())?;
        write_grants(&destination.join("world"), &GrantSet::default())?;
        manifest
    };
    let action = if known.iter().any(|app| app == &manifest.name) {
        "re-installed"
    } else {
        "installed"
    };
    println!(
        "{action} {} {} in the global catalogue at {}",
        manifest.name,
        manifest.version,
        catalogue.global_app_dir(&manifest.name)?.display()
    );
    println!(
        "  add it to a site with `ferris site install-app {} --site <site>`",
        manifest.name
    );
    Ok(())
}

/// Wrap a bare component in a minimal package and install it globally.
fn install_component(
    catalogue: &SiteManager,
    source: &Path,
    name: Option<&str>,
) -> Result<AppManifest> {
    let bytes = fs::read(source)
        .with_context(|| format!("cannot read the component {}", source.display()))?;
    if !ferris_wasm::is_component(&bytes) {
        anyhow::bail!(
            "{} is not a Ferris component (a WebAssembly component of the `ferris-app` world)",
            source.display()
        );
    }
    let name = match name {
        Some(name) => name.to_string(),
        None => source
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow::anyhow!("cannot derive an app name from {}", source.display())
            })?,
    };
    if !is_valid_app_name(&name) {
        anyhow::bail!("invalid app name `{name}`: use letters, digits, `-` or `_` (at most 64)");
    }
    validate_component(source, &name)?;

    let package = std::env::temp_dir().join(format!("ferris-app-{}", uuid::Uuid::new_v4()));
    let result = build_package(&package, source, &name).and_then(|()| {
        catalogue
            .install_global_app(&package)
            .map_err(anyhow::Error::from)
    });
    // Best effort: a leftover temporary directory must never fail the command,
    // and the install result is what the caller reports.
    let _ = fs::remove_dir_all(&package);
    result
}

/// Prove the component belongs to the contract this binary links.
///
/// `is_component` only checks the binary shape; a component built against
/// `ferris:framework@0.2.0` passes that and then aborts the site's boot with
/// `component imports instance ... not found in the linker`. Instantiating it
/// against an in-memory host moves the failure to the install command, before
/// the catalogue (and every site that will copy it) is touched.
pub(super) fn validate_component(path: &Path, name: &str) -> Result<serde_json::Value> {
    // `plugin_boot::load` parks a worker with `block_in_place`, which a
    // current-thread runtime rejects; the runtime must be multi-threaded.
    let rt = crate::runtime::block_on_runtime(Some(2))?;
    let pool = rt.block_on(ferris_db::memory_db())?;
    // The guest's `filesystem` capability becomes a WASI preopen, so a store
    // with no world refuses to instantiate every app that uses it (the
    // scaffold does). Give the probe a throwaway world, then delete it.
    let worlds = std::env::temp_dir().join(format!("ferris-validate-{}", uuid::Uuid::new_v4()));
    ferris_core::plugins::ensure_world_at(&worlds, name)?;
    let store = super::plugin_boot::new_store(pool, Some(&worlds))?;
    let result = super::plugin_boot::load(
        &rt,
        &store,
        &super::plugin_boot::AppComponent {
            name: name.to_string(),
            path: path.to_path_buf(),
        },
    )
    .with_context(|| {
        format!(
            "`{}` is not compatible with this ferris binary; rebuild it against \
             `wit/ferris.wit` from the same release",
            path.display()
        )
    });
    let described = result.and_then(|()| store.describe_json_for(name));
    let _ = fs::remove_dir_all(&worlds);
    described
        .with_context(|| format!("`{}` did not describe itself", path.display()))
        .and_then(|json| {
            serde_json::from_str(&json)
                .with_context(|| format!("`{}` described an unreadable projection", path.display()))
        })
}

/// Assemble the one-app package `install_global_app` expects.
fn build_package(package: &Path, source: &Path, name: &str) -> Result<()> {
    fs::create_dir_all(package).with_context(|| format!("cannot create {}", package.display()))?;
    fs::copy(source, package.join(format!("{name}.wasm")))
        .with_context(|| format!("cannot copy {} into the package", source.display()))?;
    // `version = "0.1.0"` is the component's own version being unknown here:
    // `plugin.toml` is the only place a version is declared, and a bare `.wasm`
    // carries none. The operator overrides it by installing a real package.
    fs::write(
        package.join("plugin.toml"),
        format!("name = \"{name}\"\nversion = \"0.1.0\"\n"),
    )
    .with_context(|| format!("cannot write the manifest in {}", package.display()))?;
    let world = package.join("world");
    for dir in WORLD_DIRS {
        fs::create_dir_all(world.join(dir))
            .with_context(|| format!("cannot create {}", world.display()))?;
    }
    Ok(())
}

fn list_catalogue(catalogue: &SiteManager, site: Option<&str>) -> Result<()> {
    let apps = catalogue.global_apps()?;
    println!("global catalogue: {}", catalogue.apps_root().display());
    if apps.is_empty() {
        println!("  (empty; run `ferris app install <package>`)");
    }
    let sites = catalogue.list()?;
    for app in apps {
        let version = catalogue
            .app_manifest(&app)
            .map_or_else(|_| "?".to_string(), |manifest| manifest.version);
        let installed: Vec<&str> = sites
            .iter()
            .filter(|entry| entry.apps.iter().any(|name| name == &app))
            .map(|entry| entry.name.as_str())
            .collect();
        println!(
            "  {:<24} {:<10} {}",
            app,
            version,
            if installed.is_empty() {
                "not installed in any site".to_string()
            } else {
                format!("sites: {}", installed.join(", "))
            }
        );
    }
    if let Some(site) = site {
        println!();
        list_site(catalogue, site)?;
    }
    Ok(())
}

/// The apps installed in one site, with their instance directory.
fn list_site(catalogue: &SiteManager, site: &str) -> Result<()> {
    let config = catalogue.get(site)?;
    let apps = catalogue.installed_apps(site)?;
    println!("site {} ({})", config.name, config.host_name);
    if apps.is_empty() {
        println!("  (no apps installed)");
        return Ok(());
    }
    for app in apps {
        let instance = catalogue.instance_world(site, &app)?;
        println!("  {:<24} {}", app, instance.display());
    }
    Ok(())
}

/// Remove an app from the catalogue, refusing while a site still installs it.
///
/// Deleting a template that a site's `apps.txt` still names would leave that
/// site unable to reinstall the app and unable to boot with the same config, so
/// the command lists the sites and asks for an explicit uninstall first.
fn remove_from_catalogue(catalogue: &SiteManager, name: &str, purge: bool) -> Result<()> {
    if !is_valid_app_name(name) {
        anyhow::bail!("invalid app name `{name}`");
    }
    let dir = catalogue.global_app_dir(name)?;
    if !dir.is_dir() {
        anyhow::bail!(
            "app `{name}` is not in the global catalogue ({})",
            catalogue.apps_root().display()
        );
    }
    let installed: Vec<String> = catalogue
        .list()?
        .into_iter()
        .filter(|site| site.apps.iter().any(|app| app == name))
        .map(|site| site.name)
        .collect();
    if !installed.is_empty() {
        anyhow::bail!(
            "app `{name}` is still installed in: {}. Run \
             `ferris site uninstall-app {name} --site <site>` for each of them first",
            installed.join(", ")
        );
    }
    if purge {
        // The catalogue holds a template, not site data: there is nothing to
        // purge here, and saying nothing would make the flag look effective.
        println!(
            "note: --purge only affects a site's instance; the global catalogue has no data to delete"
        );
    }
    fs::remove_dir_all(&dir).with_context(|| format!("cannot remove {}", dir.display()))?;
    println!("removed {name} from the global catalogue");
    Ok(())
}

// ---------------------------------------------------------------------------
// Legacy single scope (`[apps] wasm_dir`)
// ---------------------------------------------------------------------------

/// List every component and whether the current allow-list publishes it.
fn list(dir: &Path, enabled: &[String]) -> Result<()> {
    let found = discover(dir, &[])?;
    if found.is_empty() {
        println!("no WASM apps in {}", dir.display());
        return Ok(());
    }
    let all = enabled.is_empty();
    for app in found {
        let on = all || enabled.iter().any(|name| name == &app.name);
        let size = fs::metadata(&app.path).map_or(0, |meta| meta.len());
        println!(
            "{}  {:<24} {:>8} B  {}",
            if on { "enabled " } else { "disabled" },
            app.name,
            size,
            app.path.display()
        );
    }
    for name in enabled {
        if !dir.join(format!("{name}.wasm")).is_file()
            && !dir.join(name).join(format!("{name}.wasm")).is_file()
        {
            println!("missing   {name:<24} enabled in [apps] but no component on disk");
        }
    }
    Ok(())
}

fn install(
    dir: &Path,
    config_path: &Path,
    enabled: &[String],
    source: &Path,
    name: Option<&str>,
    options: &InstallOptions<'_>,
) -> Result<()> {
    // A package directory keeps its manifest and signature in the flat layout
    // too: trust is checked before the first byte is published.
    if source.is_dir() {
        let verified = verify_source(source, options)?;
        let package = AppManifest::read(source)?;
        let requested = verified
            .as_ref()
            .map_or_else(GrantSet::default, |verified| verified.grants.clone());
        let approved = approve_grants(&package.name, &requested, options)?;
        let bytes = fs::read(&package.wasm)
            .with_context(|| format!("cannot read {}", package.wasm.display()))?;
        if !ferris_wasm::is_component(&bytes) {
            anyhow::bail!(
                "{} is not a Ferris component (a WebAssembly component of the `ferris-app` world)",
                package.wasm.display()
            );
        }
        publish_component(dir, &package.name, &bytes)?;
        ensure_world_at(dir, &package.name)?;
        let world = dir.join(&package.name);
        if verified.is_some() {
            fs::copy(
                source.join(trust::SIGNATURE_FILE),
                world.join(trust::SIGNATURE_FILE),
            )
            .with_context(|| format!("cannot copy the signature into {}", world.display()))?;
        }
        write_grants(&world, &approved)?;
        return finish_install(dir, config_path, enabled, &package.name);
    }

    if !options.allow_unsigned {
        anyhow::bail!(
            "{} is a bare component with no signed manifest. Install a package \
             directory (with plugin.toml, <app>.wasm and app.sig), or pass \
             --allow-unsigned to accept an unattributable component",
            source.display()
        );
    }
    let bytes = fs::read(source)
        .with_context(|| format!("cannot read the component {}", source.display()))?;
    if !ferris_wasm::is_component(&bytes) {
        anyhow::bail!(
            "{} is not a Ferris component (a WebAssembly component of the `ferris-app` world)",
            source.display()
        );
    }
    let name = match name {
        Some(name) => name.to_string(),
        None => source
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow::anyhow!("cannot derive an app name from {}", source.display())
            })?,
    };
    if !is_valid_app_name(&name) {
        anyhow::bail!("invalid app name `{name}`: use letters, digits, `-` or `_` (at most 64)");
    }
    publish_component(dir, &name, &bytes)?;
    ensure_world_at(dir, &name)?;
    write_grants(&dir.join(&name), &GrantSet::default())?;
    finish_install(dir, config_path, enabled, &name)
}

/// Write the component with a temporary file and a rename.
fn publish_component(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let target = dir.join(format!("{name}.wasm"));
    // Write-then-rename: a crash mid-copy never leaves a half component that
    // the next boot would try to compile.
    let temporary = dir.join(format!(".{name}.wasm.tmp"));
    fs::write(&temporary, bytes)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    fs::rename(&temporary, &target)
        .with_context(|| format!("cannot publish {}", target.display()))?;
    Ok(())
}

fn finish_install(dir: &Path, config_path: &Path, enabled: &[String], name: &str) -> Result<()> {
    if !enabled.is_empty() && !enabled.iter().any(|item| item == name) {
        add_enabled(config_path, name)?;
        println!("installed {name} and added it to [apps] enabled");
    } else {
        println!(
            "installed {name} at {}",
            dir.join(format!("{name}.wasm")).display()
        );
    }
    Ok(())
}

fn remove(
    dir: &Path,
    config_path: &Path,
    enabled: &[String],
    name: &str,
    purge: bool,
) -> Result<()> {
    if !is_valid_app_name(name) {
        anyhow::bail!("invalid app name `{name}`");
    }
    let flat = dir.join(format!("{name}.wasm"));
    let nested = dir.join(name).join(format!("{name}.wasm"));
    let mut removed = false;
    for path in [&flat, &nested] {
        if path.is_file() {
            fs::remove_file(path).with_context(|| format!("cannot remove {}", path.display()))?;
            removed = true;
        }
    }
    let world = dir.join(name);
    if purge {
        if world.is_dir() {
            fs::remove_dir_all(&world)
                .with_context(|| format!("cannot remove {}", world.display()))?;
            removed = true;
            println!("purged the world of {name} (data, files and secrets)");
        }
    } else if world.is_dir() {
        println!(
            "kept the world at {}; use --purge to delete its data",
            world.display()
        );
    }
    if enabled.iter().any(|item| item == name) {
        remove_enabled(config_path, name)?;
        println!("removed {name} from [apps] enabled");
    }
    if !removed {
        anyhow::bail!("app `{name}` has no component in {}", dir.display());
    }
    println!("removed {name}");
    Ok(())
}

fn set_enabled(
    dir: &Path,
    config_path: &Path,
    enabled: &[String],
    name: &str,
    wanted: bool,
) -> Result<()> {
    if !is_valid_app_name(name) {
        anyhow::bail!("invalid app name `{name}`");
    }
    if wanted {
        if enabled.is_empty() {
            println!(
                "{name} is already enabled: `[apps] enabled` is empty, which publishes every component"
            );
            return Ok(());
        }
        if enabled.iter().any(|item| item == name) {
            println!("{name} is already enabled");
            return Ok(());
        }
        if !dir.join(format!("{name}.wasm")).is_file()
            && !dir.join(name).join(format!("{name}.wasm")).is_file()
        {
            anyhow::bail!(
                "no component for `{name}` in {}; run `ferris app install` first",
                dir.display()
            );
        }
        add_enabled(config_path, name)?;
        println!("enabled {name}");
        return Ok(());
    }

    // Empty means every component. Materialise that set first, so the list
    // really describes what is published from now on.
    let mut next: Vec<String> = if enabled.is_empty() {
        discover(dir, &[])?
            .into_iter()
            .map(|app| app.name)
            .collect()
    } else {
        enabled.to_vec()
    };
    if !next.iter().any(|item| item == name) {
        println!("{name} is already disabled");
        return Ok(());
    }
    next.retain(|item| item != name);
    write_enabled(config_path, &next)?;
    println!("disabled {name}");
    Ok(())
}

fn add_enabled(config_path: &Path, name: &str) -> Result<()> {
    let mut doc = read_document(config_path)?;
    let mut list = enabled_array(&doc);
    if !list.iter().any(|item| item == name) {
        list.push(name.to_string());
    }
    write_array(&mut doc, &list)?;
    save(config_path, &doc)
}

fn remove_enabled(config_path: &Path, name: &str) -> Result<()> {
    let mut doc = read_document(config_path)?;
    let mut list = enabled_array(&doc);
    list.retain(|item| item != name);
    write_array(&mut doc, &list)?;
    save(config_path, &doc)
}

fn write_enabled(config_path: &Path, names: &[String]) -> Result<()> {
    let mut doc = read_document(config_path)?;
    write_array(&mut doc, names)?;
    save(config_path, &doc)
}

fn read_document(path: &Path) -> Result<DocumentMut> {
    if !path.exists() {
        anyhow::bail!(
            "config file not found: {}. Run this from a project root or pass --config.",
            path.display()
        );
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    text.parse::<DocumentMut>()
        .map_err(|error| anyhow::anyhow!("{} is not valid TOML: {error}", path.display()))
}

fn enabled_array(doc: &DocumentMut) -> Vec<String> {
    doc.get("apps")
        .and_then(Item::as_table)
        .and_then(|apps| apps.get("enabled"))
        .and_then(Item::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn write_array(doc: &mut DocumentMut, names: &[String]) -> Result<()> {
    let mut array = Array::new();
    for name in names {
        array.push(name.as_str());
    }
    let apps = doc
        .entry("apps")
        .or_insert_with(|| Item::Table(Table::new()));
    let table = apps
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("`[apps]` in the config is not a table"))?;
    table["enabled"] = toml_edit::value(array);
    Ok(())
}

fn save(path: &Path, doc: &DocumentMut) -> Result<()> {
    fs::write(path, doc.to_string()).with_context(|| format!("cannot write {}", path.display()))
}
/// The path a world would occupy (used by the tests).
#[cfg(test)]
fn world(dir: &Path, name: &str) -> std::path::PathBuf {
    dir.join(name)
}

#[cfg(test)]
mod tests;
