//! Non-code files: `.gitignore` and `README.md`.

use anyhow::Result;
use std::fs;
use std::path::Path;

pub(super) fn write_gitignore(project_dir: &Path) -> Result<()> {
    fs::write(
        project_dir.join(".gitignore"),
        "/target\nCargo.lock\n*.db\n.env\n",
    )?;
    Ok(())
}

/// The bench repository ignores build output and, above all, `sites/`.
///
/// A site's `site_config.json` carries the database password and its instances
/// carry uploaded files and the app's secrets; none of that belongs in git.
pub(super) fn write_bench_gitignore(root: &Path) -> Result<()> {
    let path = root.join(".gitignore");
    if path.exists() {
        return Ok(());
    }
    fs::write(
        path,
        "/target\n/apps/*/target\n*.db\n.env\n\n# Deployments: credentials, data, files and secrets\n/sites/\n\n# App dev worlds (the exported template lives in apps/<app>/world)\n/apps/*/data/\n/apps/*/files/\n/apps/*/secrets/\n/apps/*/world/data/\n/apps/*/world/files/\n/apps/*/world/secrets/\n",
    )?;
    Ok(())
}

pub(super) fn write_readme(project_dir: &Path, name: &str) -> Result<()> {
    fs::write(project_dir.join("README.md"), render_readme(name))?;
    Ok(())
}

pub(super) fn render_readme(name: &str) -> String {
    format!(
        r#"# {name}

A Ferris app: a WIT/WASM component. It compiles to `wasm32-wasip2` and runs as
a plugin inside the `ferris` binary — the framework hosts every service (the
database, the file store, sockets, TLS certificates, schedules, delivery), and
the plugin never touches any of them directly.

The app lives at `apps/{name}/` inside its bench. Run the commands from the
bench root so the catalogue (`apps/`) and the sites (`sites/`) are the same
ones `ferris build` and `ferris site` resolve:

```bash
ferris dev --app {name}     # build + hot-reload this app
ferris build --app {name}   # build apps/{name}/{name}.wasm (the catalogue package)
ferris migrate              # apply the framework schema
```

`ferris build` also writes `apps/{name}/plugin.toml` and a `world/` template,
which is exactly the package `ferris site install-app {name}` copies into a
site. The site's copy is independent: upgrading the catalogue never changes a
site's pinned component until it is reinstalled with `--force`.

## What the app declares

The app contract is `ferris:framework@0.3.0` (`wit/ferris.wit`, six interfaces:
`types`, `data`, `schema`, `io`, `platform` and `descriptor`). Nothing is
configured by the host operator: `describe()` declares, and the declaration is
the grant.

The scaffold lists `routes`, `docstore-read`, `docstore-write`, `filesystem`,
`streams`, `scheduler`, `notifications` and `pubsub`. Every entry except
`streams` is **primordial** — the app always has it, and listing it is only
documentation. `streams` is the one **specific** grant the scaffold needs: a
specific capability widens a host boundary, and any call outside the resulting
set fails with `permission-denied`. Drop a route and you can drop its
capability; add a service and you add both the capability and the code that
uses it. `platform::grants()` reports the effective set.

`filesystem` is the app's own world (`apps/<app>/` in development,
`sites/<site>/plugins/<app>/` once installed, both mounted at `/world`): the app
writes its files and settings there, and no blob/file host API exists.

`describe().routes` is the other half of the declaration: `handle-request` is
only called for an app that declares a route, each dispatch names the matched
`route-id` (API methods arrive as `method:<name>`), and the guest parses
`http-request.path` itself. `%stream` routes carry a listener instead, and their
connections arrive through `handle-stream` (frames) and `handle-stream-event`
(`opened` / `closed` / `error`).

## Permissions (`docperm.toml`)

A WASM descriptor declares DocTypes but has no permissions field. The
scaffold ships `docperm.toml` next to `plugin.toml`; `ferris site
install-app` applies its `[[permissions]]` rows to that site's database (the
same rows `ferris role-permissions set` writes). The default grants the
administrator full rights on `Task`; add a role to let other users in.

## Capabilities: declared, not configured

The capabilities that widen a host boundary are `http-unrestricted`,
`tls-client`, `tls-server`, `streams`, `streams-listen` and `pki`, plus
`overrides` (replacing a framework API). None has an operator switch: declaring
one in `describe().capabilities` is the request and the grant.

A grant is **not** the same question as "does this binary have the subsystem at
all". The framework is built from optional verticals (`api`, `web`, `realtime`,
`jobs`, `scheduler`, `email`, `files`, `print`, `report`, `integrations`,
`wasm`, `full`), and a host may be compiled without one. `platform::features()`
reports what the running host actually compiled; a surface it did not compile
answers the typed `unsupported` failure, so an app checks the list before
exposing an affordance.
"#
    )
}
