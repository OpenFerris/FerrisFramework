//! Deploy command
//!
//! Every target here used to end in `info!("Deployment completed successfully!")`
//! whether or not anything had been deployed: `--target serverless` did nothing
//! but log a warning, `--target kubernetes` without a `k8s/` directory logged
//! "skipping manifest application" and then reported success, and
//! `--target binary` copied nothing at all — it looked for
//! `target/release/ferris-app`, and a Ferris app is a `cdylib` for
//! `wasm32-wasip2`, which produces no native binary under that name. The
//! runtime `CARGO_PKG_NAME` lookup that chose that name was always `None`.
//!
//! What can honestly be deployed from an app project is the plugin itself. The
//! server that loads it is the Ferris binary, not something this crate builds,
//! so a Dockerfile generated from this crate alone could never run the app — and
//! the one it wrote could never build it either (it copied a `Cargo.lock` the
//! project ignores and a `migrations/` that `ferris new` never creates). So the
//! file is no longer generated, and each target either does its work or fails.

use super::*;
use crate::config::CliConfig;
use crate::project;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

pub fn run(args: super::DeployArgs, config: &CliConfig) -> Result<()> {
    project::check_project_exists(&config.source.project_dir)?;

    if !args.skip_build {
        info!("Building the plugin...");
        super::build::build_plugin(&config.source.project_dir, "release")?;
    }

    match args.target {
        super::DeployTarget::Docker => deploy_docker(&args)?,
        super::DeployTarget::Kubernetes => deploy_kubernetes(&args)?,
        super::DeployTarget::Serverless => deploy_serverless(&args)?,
        super::DeployTarget::Binary => deploy_binary(&config.source.project_dir, &args)?,
    }

    Ok(())
}

fn deploy_docker(args: &super::DeployArgs) -> Result<()> {
    if !std::path::Path::new("Dockerfile").exists() {
        anyhow::bail!(
            "no Dockerfile in the project, and none will be written for you: the image \
             needs the Ferris server binary (which comes from a Ferris install, not from \
             this crate) plus this project's plugin at plugins/. Write a Dockerfile that \
             does both and re-run, or use --target binary."
        );
    }

    let image = format!("ferris-app:{}", args.env);
    // The build context is the whole project. This command does not choose it,
    // but an operator should know when a credential is about to be sent.
    warn_private_context(Path::new("."));
    // `registry.example.com` was a placeholder: tagging and pushing it invented a
    // registry that has nothing to do with the user's. Push only when asked for,
    // with the registry the user names.
    let Some(registry) = args.registry.as_deref() else {
        info!("Built {image} locally. Pass --registry to tag and push it.");
        return Ok(());
    };

    run_tool("docker", &["build", "-t", &image, "."])?;
    let tagged = format!("{registry}/{image}");
    run_tool("docker", &["tag", &image, &tagged])?;
    run_tool("docker", &["push", &tagged])?;
    info!("Pushed {tagged}");
    Ok(())
}

fn deploy_kubernetes(args: &super::DeployArgs) -> Result<()> {
    run_tool("kubectl", &["version", "--client"])?;

    if !std::path::Path::new("k8s").exists() {
        // Before: warn, then "Kubernetes deployment completed", then "Deployment
        // completed successfully" — three messages for a deployment that never
        // started.
        anyhow::bail!(
            "no k8s/ directory in the project: there are no manifests to apply. \
             Create it, or use --target binary."
        );
    }

    run_tool("kubectl", &["apply", "-f", "k8s/", "-n", &args.env])?;
    info!("Applied k8s/ to namespace {}", args.env);
    Ok(())
}

fn deploy_serverless(_args: &super::DeployArgs) -> Result<()> {
    anyhow::bail!(
        "--target serverless is not implemented. `ferris deploy` knows how to build an \
         image with docker, apply k8s/ manifests, or copy the plugin to a directory; a \
         serverless deployment needs a provider and an adapter that do not exist yet."
    )
}

/// Where `--target binary` puts the plugin: `--to`, else `/opt/ferris/<env>`.
///
/// `env` is three things at once — a docker image tag, a k8s namespace and this
/// directory name — and only the last one cared what it contained: `--env
/// ../../etc` deployed to `/etc`, because `/opt/ferris/../../etc` is `/etc`. So it
/// has to be one path segment. `--to` is a path by definition and is left alone.
fn deploy_dir_for(env: &str, to: Option<&str>) -> Result<PathBuf> {
    if let Some(dir) = to {
        return Ok(PathBuf::from(dir));
    }
    let usable = !env.is_empty() && env != "." && env != ".." && !env.contains(['/', '\\', '\0']);
    if !usable {
        anyhow::bail!(
            "--env {env:?} is not a directory name. It becomes /opt/ferris/<env>, so it \
             has to be a single segment: pass a name like `staging`, or --to for the \
             directory itself."
        );
    }
    Ok(PathBuf::from(format!("/opt/ferris/{env}")))
}

/// Copy the plugin and the config to `/opt/ferris/<env>` (or `--to`).
fn deploy_binary(project_dir: &Path, args: &super::DeployArgs) -> Result<()> {
    let plugin_name = super::build::package_name(project_dir)?;
    let plugin = super::build::plugin_path(project_dir, &plugin_name);
    if !plugin.is_file() {
        anyhow::bail!(
            "no plugin at {}. Run `ferris build` first, or drop --skip-build.",
            plugin.display()
        );
    }

    let deploy_dir = deploy_dir_for(&args.env, args.to.as_deref())?;
    std::fs::create_dir_all(&deploy_dir).map_err(|e| {
        anyhow::anyhow!(
            "could not create {}: {e}. /opt needs root; pass --to to deploy elsewhere.",
            deploy_dir.display()
        )
    })?;

    let target = super::build::plugin_path(&deploy_dir, &plugin_name);
    std::fs::create_dir_all(target.parent().expect("a file has a parent"))?;
    std::fs::copy(&plugin, &target)?;
    info!("Plugin copied to {}", target.display());

    let config_src = project_dir.join("config");
    if config_src.is_dir() {
        let deployed_config = deploy_dir.join("config");
        let copied = copy_deployable(&config_src, &deployed_config)?;
        info!(
            "Config copied to {} ({copied} file(s); .env, *.pem and *.key were not copied)",
            deployed_config.display()
        );
    }

    // The plugin is not a program: something else has to load it. Saying so is the
    // difference between a deployed artifact and a deployed app.
    info!(
        "Deployed the plugin only. Run the Ferris server against it, e.g. \
         `ferris serve --api` from {}, with plugins/ next to the config.",
        deploy_dir.display()
    );
    Ok(())
}

/// Copy `config/` into the deploy directory, leaving credentials behind.
///
/// The artifact goes to `/opt/ferris/<env>` (or `--to`), which is a place other
/// accounts and a container image can reach. An `.env`, a private key or a
/// certificate key is not configuration to ship: it either belongs in the
/// platform's secret store or is injected with `FERRIS_*` at runtime. Skipping
/// them is explicit and logged, not a silent omission.
fn copy_deployable(src: &Path, dst: &Path) -> Result<usize> {
    std::fs::create_dir_all(dst)?;
    let mut copied = 0;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let kind = entry.file_type()?;

        if is_private_file(&from) {
            warn!("Not copying {} into the deploy artifact", from.display());
            continue;
        }
        if kind.is_dir() {
            copied += copy_deployable(&from, &to)?;
        } else if kind.is_file() {
            std::fs::copy(&from, &to)?;
            copied += 1;
        }
        // A symlink is neither followed nor copied: the deploy artifact must
        // not point back into the project.
    }
    Ok(copied)
}

/// Files that carry credentials and must not be deployed.
fn is_private_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    name == ".env" || name.starts_with(".env.") || extension == "pem" || extension == "key"
}

/// Tell the operator when the docker build context contains a credential.
///
/// `docker build .` sends the whole project unless a `.dockerignore` says
/// otherwise, and that file belongs to the project, not to this command.
fn warn_private_context(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_private_file(&path) {
            warn!(
                "{} is in the docker build context; add it to .dockerignore",
                path.display()
            );
        }
    }
}

fn run_tool(program: &str, args: &[&str]) -> Result<()> {
    let status = std::process::Command::new(program).args(args).status();
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(anyhow::anyhow!(
            "`{program} {}` failed with {status}",
            args.join(" ")
        )),
        Err(e) => Err(anyhow::anyhow!(
            "could not run {program}: {e}. Is it installed?"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_environment_is_not_a_path() {
        assert_eq!(
            deploy_dir_for("production", None).unwrap(),
            PathBuf::from("/opt/ferris/production")
        );

        for hostile in [
            "../../etc",
            "..",
            ".",
            "a/b",
            "..\\..\\windows",
            "",
            "sub/dir",
        ] {
            let err = deploy_dir_for(hostile, None)
                .expect_err(&format!("--env {hostile:?} must not be a directory"));
            assert!(
                err.to_string().contains("single segment"),
                "{hostile:?}: {err}"
            );
        }
    }

    #[test]
    fn to_is_a_path_and_is_left_alone() {
        assert_eq!(
            deploy_dir_for("../../etc", Some("/srv/app")).unwrap(),
            PathBuf::from("/srv/app")
        );
        assert_eq!(
            deploy_dir_for("production", Some("relative/dir")).unwrap(),
            PathBuf::from("relative/dir")
        );
    }

    #[test]
    fn serverless_says_it_does_not_exist() {
        let err = deploy_serverless(&super::super::DeployArgs {
            target: super::super::DeployTarget::Serverless,
            env: "production".into(),
            skip_build: true,
            to: None,
            registry: None,
        })
        .expect_err("nothing was implemented");
        assert!(err.to_string().contains("not implemented"), "{err}");
    }

    #[test]
    fn a_missing_binary_is_named() {
        let dir = std::env::temp_dir().join(format!("ferris-deploy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"my-app\"\n").unwrap();
        let err = deploy_binary(
            &dir,
            &super::super::DeployArgs {
                target: super::super::DeployTarget::Binary,
                env: "production".into(),
                skip_build: true,
                to: None,
                registry: None,
            },
        )
        .expect_err("there is no plugin in an empty directory");
        assert!(err.to_string().contains("ferris build"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn deployment_never_copies_private_files() {
        let dir =
            std::env::temp_dir().join(format!("ferris-deploy-private-{}", std::process::id()));
        let src = dir.join("config");
        let dst = dir.join("out");
        std::fs::create_dir_all(src.join("certs")).unwrap();
        std::fs::write(src.join("ferris.toml"), "[server]\nport = 8000\n").unwrap();
        std::fs::write(src.join(".env"), "JWT_SECRET=leaked\n").unwrap();
        std::fs::write(src.join(".env.local"), "DATABASE_URL=leaked\n").unwrap();
        std::fs::write(src.join("serve.pem"), "-----BEGIN CERTIFICATE-----").unwrap();
        std::fs::write(src.join("serve.key"), "-----BEGIN PRIVATE KEY-----").unwrap();
        std::fs::write(
            src.join("certs").join("ca.pem"),
            "-----BEGIN CERTIFICATE-----",
        )
        .unwrap();
        std::fs::write(src.join("certs").join("notes.txt"), "public\n").unwrap();

        let copied = copy_deployable(&src, &dst).unwrap();

        assert!(dst.join("ferris.toml").is_file());
        assert!(dst.join("certs").join("notes.txt").is_file());
        for private in [
            ".env",
            ".env.local",
            "serve.pem",
            "serve.key",
            "certs/ca.pem",
        ] {
            assert!(
                !dst.join(private).exists(),
                "{private} reached the artifact"
            );
        }
        assert_eq!(copied, 2, "only the TOML and the notes are deployable");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_tool_is_named() {
        let err = run_tool("ferris-no-such-tool", &["--version"]).expect_err("not installed");
        assert!(err.to_string().contains("ferris-no-such-tool"), "{err}");
        assert!(err.to_string().contains("Is it installed?"), "{err}");
    }
}
