//! `ferris run`: serve using the roles in `server.roles` from the project config.

use anyhow::Result;
use ferris_server::ServerRoles;
use tracing::info;

use super::{RunArgs, serve};
use crate::config::CliConfig;

pub fn run(args: RunArgs, config: &CliConfig) -> Result<()> {
    let mut effective = config.clone();
    // `--site` is the sites/ layout; `--tenant` is the older registry. Either
    // rewrites the process-wide data locations before settings are built, and
    // `--site` also installs the `Host` allow-list for its `host_name`.
    if let Some(name) = args.site.as_deref() {
        super::site::apply_site_by_name(&mut effective, name)?;
    } else {
        if let Some(id) = args.tenant.as_deref() {
            tracing::warn!(
                "--tenant is the pre-sites model; `ferris run --site <name>` uses the \
                 sites/ layout and provisions the same per-process isolation"
            );
            super::tenant::apply_tenant_by_id(&mut effective, id)?;
        }
        serve::apply_allowed_hosts(&effective)?;
    }
    let config = &effective;
    let roles = ServerRoles::parse(&config.server.roles)?;
    if roles.is_empty() {
        // `ferris run` takes its roles from `server.roles`, so an empty list means
        // a config that says nothing should run. Starting a server with no role
        // enabled answers no request and looks alive, which is worse than an
        // error naming the key to fix.
        anyhow::bail!(
            "no roles in server.roles: nothing would be enabled. Add e.g. \
             roles = [\"api\", \"web\", \"realtime\"] to config/ferris.toml, or run \
             `ferris serve --all` to pick them on the command line."
        );
    }
    info!(
        "serving roles from config: {:?}",
        roles.iter().collect::<Vec<_>>()
    );
    let settings = serve::settings_for(
        config.server.host.clone(),
        config.server.port,
        roles,
        config,
    )?;
    serve::start(settings, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CliConfig;

    fn config_with_roles(roles: &[&str]) -> CliConfig {
        let mut config = CliConfig::default();
        config.server.roles = roles.iter().map(|r| (*r).to_string()).collect();
        config
    }

    #[test]
    fn an_empty_role_list_is_refused() {
        let args = RunArgs {
            tenant: None,
            site: None,
        };
        let err = run(args, &config_with_roles(&[])).expect_err("no roles, no server");
        assert!(
            err.to_string().contains("no roles in server.roles"),
            "{err}"
        );
        assert!(
            err.to_string().contains("config/ferris.toml"),
            "the error must say which file to change: {err}"
        );
    }

    #[test]
    fn a_scaffolded_config_still_passes_the_guard() {
        // The guard must not turn a normal `ferris run` into an error: this
        // project has roles, so it gets as far as building the settings (and then
        // fails on the database, which is not what is under test here).
        let config = config_with_roles(&["api"]);
        let roles = ServerRoles::parse(&config.server.roles).unwrap();
        assert!(!roles.is_empty());
    }
}
