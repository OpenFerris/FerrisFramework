//! `ferris init`: turn an **existing** directory into a bench.
//!
//! The bench is the unit of deployment, as in Frappe: one `config/ferris.toml`,
//! one `apps/` catalogue and one `sites/` tree. `init` only creates that
//! skeleton; it does not delete anything and does not scaffold an app, so it is
//! safe to run over a folder that already holds a README, a `.git` or an
//! editor configuration.
//!
//! Everything after this is contextual: `ferris new` at the bench root fills
//! `apps/`, `ferris site new` fills `sites/`, and the site/app commands only
//! appear where they apply (see [`crate::context`]).

use anyhow::Result;
use ferris_core::site::SiteManager;
use tracing::info;

use crate::commands::InitArgs;
use crate::config::CliConfig;

/// Create the bench skeleton in an existing directory.
///
/// # Errors
///
/// The path is not an existing directory, a bench already exists there, or the
/// layout cannot be written.
pub fn run(args: InitArgs, config: &CliConfig) -> Result<()> {
    let root = args
        .path
        .clone()
        .unwrap_or_else(|| config.source.project_dir.clone());

    if !root.is_dir() {
        anyhow::bail!(
            "`{}` is not an existing directory: create it first, then run \
             `ferris init {}`",
            root.display(),
            root.display()
        );
    }

    let config_path = root.join("config").join("ferris.toml");
    if config_path.is_file() {
        anyhow::bail!(
            "`{}` is already a Ferris bench (found `{}`); create an app with \
             `ferris new <name>` or a site with `ferris site new <name>`",
            root.display(),
            config_path.display()
        );
    }

    let created = super::new::write_bench_config(&root)?;
    let manager = SiteManager::new(root.join("sites"), root.join("apps"));
    if let Err(error) = manager.init_bench() {
        if created {
            let _ = std::fs::remove_file(&config_path);
        }
        return Err(error.into());
    }
    super::new::write_bench_gitignore(&root)?;

    info!("Bench initialised at {}", root.display());
    info!("  config/ferris.toml      one config for every app and site");
    info!("  apps/                   the app catalogue (`ferris new <name>`)");
    info!("  sites/                  deployments (`ferris site new <name> --admin-email ...`)");
    info!("Next steps:");
    info!("  cd {}", root.display());
    info!("  ferris new myapp");
    info!("  ferris site new myapp.local --admin-email admin@myapp.local --admin-password ...");
    info!("  ferris serve --all-sites");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CliConfig, ConfigSource};

    fn scratch(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ferris-init-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("the scratch directory is writable");
        root
    }

    fn config_for(root: &std::path::Path) -> CliConfig {
        let mut config = CliConfig::default();
        config.source = ConfigSource {
            config_path: root.join("config").join("ferris.toml"),
            project_dir: root.to_path_buf(),
            bench_root: root.to_path_buf(),
        };
        config
    }

    #[test]
    fn init_creates_the_skeleton_and_refuses_a_second_run() {
        let root = scratch("skeleton");
        let config = config_for(&root);

        run(InitArgs { path: None }, &config).expect("the bench is created");

        assert!(root.join("config/ferris.toml").is_file());
        assert!(root.join("apps").is_dir());
        assert!(root.join("sites").is_dir());
        assert!(root.join("sites/common_site_config.json").is_file());
        assert!(root.join(".gitignore").is_file());

        let error = run(InitArgs { path: None }, &config).expect_err("a bench exists");
        assert!(
            error.to_string().contains("already a Ferris bench"),
            "{error}"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn init_requires_an_existing_directory() {
        let root = scratch("missing");
        let target = root.join("not-there");
        let config = config_for(&root);

        let error = run(
            InitArgs {
                path: Some(target.clone()),
            },
            &config,
        )
        .expect_err("the directory does not exist");
        assert!(error.to_string().contains("existing directory"), "{error}");
        std::fs::remove_dir_all(root).ok();
    }
}
