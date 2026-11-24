# AGENTS.md — FerrisFramework

Operating contract for any agent (or human) working on **FerrisFramework**, the
public facade of Ferris: Desk, verticals, WASM host, server and CLI. It
describes the tree as it exists today; verify before trusting it and update it
when reality changes.

Read §0 (container rule), §1 (compaction) and §2 (sibling Core) first.

---

## 0. HARD RULE — build inside the project container

FerrisFramework builds inside the **pinned image** defined by this repository.

| Fact | Value |
|---|---|
| Image | `ferris-dev`, built from `Dockerfile` |
| Orchestration | `compose.yaml` (service `ferris`, PostgreSQL, Redis) |
| Workspace | this repository at `/workspace`; **FerrisCore at `/FerrisCore`** |
| Toolchain | `rust-toolchain.toml` pins **1.98.1** |
| Targets | `wasm32-unknown-unknown`, `wasm32-wasip2` |
| Also inside | Node 24, `wasm-bindgen-cli` 0.2.128, `cargo-leptos`, `wasm-tools` |
| User | non-root `ferris`; `USER_ID`/`GROUP_ID` are build args (default 1000) |

### Canonical way to run a command

```bash
docker compose run --rm ferris cargo check --all-targets
```

Define it once per session:

```bash
dx() { docker compose run --rm ferris sh -lc "$*"; }
dx cargo check-cli
```

Inside the devcontainer (`.devcontainer/devcontainer.json`) run the command
directly; the workspace and toolchain are the same.

### Session sanity check (run first)

```bash
docker compose run --rm ferris sh -lc 'rustc -V && cargo -V && node -v && rustup target list --installed'
```

Must print `1.98.1` for both `rustc` and `cargo`, a Node 24 version and both
WASM targets. If the image is stale, rebuild it (`docker compose build --no-cache`);
do not install a second toolchain on the host.

### Forbidden

- **Never** run `cargo`, `rustc`, `rustup`, `cargo-leptos`, `trunk`,
  `wasm-bindgen`, `node`, `npm`, `npx`, `tailwindcss` or `ferris` on the host.
- **Never** run them in another checkout or container with a different toolchain.
- **Never** hand-edit `target/` or copy build artifacts between host and
  container.
- **Never `cargo clean`**: it deletes the shared dependency cache. Remove only
  the profile you need and check `df -h .` before a large rebuild.

Allowed on the host: `git`, `docker`/`docker compose` (to run the project image),
read-only inspection and editors. Run a language server inside the devcontainer.

---

## 1. Compaction & continuity protocol

A checkpoint, handoff or summary MUST carry, in this order:

1. **The main task, verbatim and complete**, including every clause of a
   multi-part order. Do not paraphrase scope away.
2. **What is needed to avoid re-exploring**: files/symbols already read
   (`path:line`), commands run and results, artifacts changed, decisions and
   why.
3. **Every explicit user rule** (language, paths, prohibitions, scope limits)
   plus this file's rules that bear on the task. User rules outrank defaults.
4. **Current position and next step**: done, in progress, next action, blocked
   items, unverified claims.

Point at canonical docs and commits instead of pasting content. Never claim done
what you did not verify. Keep it small but lossless.

---

## 2. What this repository is

FerrisFramework is the product layer. It contains 12 crates and **depends on
FerrisCore by relative path**.

| Crate | Responsibility |
|---|---|
| `ferris-web` | The Desk application: routes, state, `DocStore` facade, entry points, CSS. |
| `ferris-realtime` | WebSocket/events. |
| `ferris-jobs` | Redis-backed queue and worker. |
| `ferris-scheduler` | Cron scheduler. |
| `ferris-files` | Object storage (S3-compatible), reference `Module` migration. |
| `ferris-email` | Email vertical (declarations + host-wired mailer). |
| `ferris-print` | Print vertical (routed module). |
| `ferris-report` | Report vertical (routed module). |
| `ferris-integrations` | OAuth/Google/LDAP/webhook DocTypes and runtime adapters. |
| `ferris-wasm` | Native wasmtime **host** + `wit/ferris.wit` component contract. |
| `ferris-server` | axum router, server roles, module composition root, feature gate. |
| `ferris-cli` | `ferris init/new/dev/serve/run/build/app/migrate/generate/deploy/tenant/shell/benchmark/info/doctor/config/role-permissions`, with the available commands selected by the working directory (bench root, site, app or none). |

Workspace: `resolver = "2"`, edition 2024, version **0.4.0**, Rust **1.98.1**,
MIT.

### Sibling Core is mandatory

Every crate that needs the engine points at the sibling checkout:

```toml
# ferris-server/Cargo.toml
ferris-core = { path = "../../FerrisCore/ferris-core" }
```

- The checkouts must be siblings: `.../ferris/FerrisCore` and
  `.../ferris/FerrisFramework`.
- `compose.yaml` mounts `../FerrisCore` at `/FerrisCore`; inside the container
  `/workspace/ferris-server/../../FerrisCore` resolves correctly.
- CI checks the framework repository out at `framework/` and `FerrisCore` at
  `FerrisCore`; from `framework/ferris-<crate>`, `../../FerrisCore` resolves to
  that sibling. Every Rust job checks out both repositories, and so does the
  `Tailwind` job (it builds `ferris-web/style/desk.css` from the Core UI
  sources).
- **Framework depends on Core, never the reverse.** A primitive belongs in Core;
  a vertical-specific piece stays here.

### Feature split is load-bearing

`default-members` is deliberately cheap and includes **5 crates**: `ferris-web`,
`ferris-realtime`, `ferris-jobs`, `ferris-scheduler`,
`ferris-integrations`. It excludes `ferris-files`, `ferris-email`,
`ferris-print`, `ferris-report`, `ferris-wasm`, `ferris-server` and
`ferris-cli`, because wasmtime/cranelift makes the build expensive.

```bash
cargo check-cli   # check -p ferris-cli -p ferris-server -p ferris-wasm --all-targets
```

`ferris-web` features: `default = []` (platform-neutral Desk typecheck), `ssr`
(native engine bridge), `wasm-runtime` (wasmtime + HTTP stack, only
`src/bin/ssr.rs`), `hydrate` (wasm32/browser). Keep the library free of
native-only imports so `unused_crate_dependencies` stays honest.

### Compile-time features and the boot matrix

`ferris-server` and `ferris-cli` expose one Cargo feature per subsystem, with
the **same names and default set**: `api` (base, always on), `web`, `realtime`,
`jobs`, `scheduler`, `email`, `files`, `print`, `report`, `integrations`, `wasm`,
plus `full` as a convenience alias. `--no-default-features --features api,web` is
the minimal server.

- A feature is compile-time. `false` removes a role/route that was compiled in
  (`ferris-server::server::gate_roles`); `true` never adds a subsystem that was
  not linked.
- A config that asks for something the binary lacks **fails at boot**, never is
  ignored.
- `ferris-server::host_features` is the one source of truth for what was
  compiled; the CLI reads it through `ferris-cli/src/features.rs` (wording and
  mismatch check). `ferris doctor` reports the matrix as critical.
- Roles are the server's business. The CLI validates `[features]` only.
- Touching the matrix means re-checking both ends:
  `cargo check -p ferris-cli --no-default-features --features api` and
  `cargo check -p ferris-cli --features full`, in addition to the default
  `cargo check --all-targets`.

### Bench, sites and the CLI context

A bench is one `config/ferris.toml`, one `apps/` catalogue and one `sites/` tree;
`ferris init` creates it in an existing directory. A site is a self-contained
directory under `sites/<site>/`: `site_config.json`, `apps.txt`, `plugins/`,
`private/` (files and, for SQLite, the database at `private/<db>`) and
`public/files/`. Nothing a site needs lives outside that directory.

`ferris-cli/src/context.rs` detects the zone from the working directory and
decides which commands exist there (`restrict` hides the rest from `--help`,
`check` rejects them, `prepare` fills the enclosing site's name and rewrites the
config before a database is opened). Adding a command means adding it to the
zone's catalogue in that module and covering it in the docs page
`cli/init-y-contexto`.

### Plugin worlds

Every enabled app owns `plugins/<app>/`: `customizations/`, `data/`, `files/`
and `secrets/`. The CLI creates the directory at boot (`serve`, `run`, `dev`);
the name is validated by `ferris_core::plugins` before it becomes a path
component. The world is **mounted** read-write at `/world` for every guest; the
preopen is the sandbox. `ferris migrate` applies every enabled world's
`customizations/*.json` into `tab_custom_field` / `tab_property_setter`.

### WIT contract and extensibility

The contract is `ferris-wasm/wit/ferris.wit` (`ferris:framework@0.4.0`; six
interfaces: `types`, `data`, `schema`, `io`, `platform`, `descriptor`). **The
WIT package version and the workspace version are the same number** in both
`Cargo.toml` files, both WIT copies and `FerrisDocs/project.json`, guarded by
`ferris-wasm/tests/wit_version.rs`.

An app declares the locales it translates (`descriptor.locales`) and ships the
messages as `locales/<tag>.toml` next to `plugin.toml`; `app install` and
`site install-app` validate and copy them, and boot merges them over the
embedded catalogues. Metadata labels are resolved server-side through the
`doctype.<X>.label` / `field.<X>.<field>.label` / `workspace.<X>.label`
convention, never by translating identifiers.

Capabilities are primordial (always granted: world, routes, outbound HTTP,
document store, config, schema, scheduler, notifications, pubsub) or specific
(granted by declaring them). The host owns every descriptor. API methods are
inheritable via `descriptor.method.overrides` + `data::call-method`, resolved by
`ferris_api::MethodRegistry` (Core).

### Native modules and the API seam

`ferris-api` (Core) defines the `Module` contract. This repository implements it
per vertical behind an `http` (or lighter `module`) feature and depends on
`ferris-api`, never the reverse. `ferris-server` is the composition root:
`router/modules.rs` builds the registry, boots it before the router and mounts
the API routes inside the shared layers.

- A module never mounts its own auth or CSRF; the host wraps it.
- `public_mount()` is the only exception and `/` is reserved.
- `Module::openapi()` contributes the full paths it mounts.
- Document notifications go through `ferris_api::notify::Notifier`.
- A module that needs background work consumes `ferris_core::ports` (queue,
  mailer), never SMTP/Redis credentials.
- `ferris-api/tests/architecture.rs` fails if the API manifest regains a direct
  vertical dependency.

---

## 3. Frontend rules (Desk / Leptos)

Applies to `ferris-web` here and to `ferris-web-ui` / `ferris-web-shared` in
Core.

- **Dependency direction**: `ferris-web → ferris-web-ui → ferris-web-shared`.
  Never invert it.
- **Reuse the global component library.** Before writing markup, check
  `ferris-web-ui/src/components/ui/` (~90 components). A new primitive goes
  *into* that crate and is exported from `ui/mod.rs`, never inline in a view.
  Hooks live in `components/hooks/`.
- **Buttons** use `<Button variant=… size=…>`; variants are declared with
  `variants!` and literal classes in `button.rs`. Never build class strings at
  runtime. Caller `class` merges via `tw_merge!` and may only override
  layout/size.
- **Colors**: only global semantic tokens (`bg-background`, `text-foreground`,
  `bg-primary`, `border-border`, status tokens, Desk chrome). Forbidden
  everywhere: `#hex`, `rgb()`, `hsl()`, `oklch()`, inline `style=`, palette
  classes with a shade (`bg-zinc-200`, `text-white`, …) and raw color words.
- **Component catalog**: colour decisions for built-ins live in
  `ferris-web/style/components.css` as `.ferris-*` classes, with the class list
  in `ferris_web_ui::catalog`. Add both together; `design_system` asserts they
  match. No literal colours in `components.css`, only `var(--token)`.
- **Do not use `dark:` for colour**: runtime themes paint `:root`; the token
  already switches.
- **CSS pipeline**: source `ferris-web/style/tailwind.css`, output
  `ferris-web/style/desk.css`,
  `cd ferris-web && node_modules/.bin/tailwindcss -i style/tailwind.css -o style/desk.css --minify`.
  Commit the regenerated `desk.css`. `@source "../../ferris-web-ui/src";` must
  stay in `tailwind.css`.
- **Metadata-driven, never hardcoded**: views read `DynMeta`/`DynField` from the
  `/api/*` contract and render generically.
- **Data access only through `DocStore`** (`ferris-web/src/api/store.rs`):
  `Server` for SSR, `Client` for hydrate. No direct `fetch`/`reqwest` in a view.
- **SSR/hydrate parity**: browser globals only in islands/effects or behind the
  client store. No `cfg(feature = "hydrate")` branches in views.
- Accessibility is part of "done": labels, roles, `aria-*`, keyboard paths,
  focus trapping, no information by color alone. Every data view handles
  loading, empty and error states.

---

## 4. Definition of Done (gates)

All commands run inside the dev container via `dx '…'`.

| Gate | Command | When |
|---|---|---|
| Format | `cargo fmt --all -- --check` | always |
| Lint | `cargo clippy --all-targets` | always (default members) |
| Typecheck | `cargo check --all-targets` | always |
| Heavy crates | `cargo check-cli` | if you touched `ferris-cli`, `ferris-server` or `ferris-wasm` |
| Feature matrix | `cargo check -p ferris-cli --no-default-features --features api` and `--features full` | if you touched `features.rs` or a `[features]` table |
| Hydrate | `cargo check -p ferris-web --features hydrate --target wasm32-unknown-unknown` | if you touched browser Desk code |
| SSR binary | `cargo check -p ferris-web --bin ferris-web-ssr --features ssr,wasm-runtime` | if you touched the SSR bridge or a `ferris-wasm` type it matches on |
| SSR | `cargo check -p ferris-web --features ssr` | if you touched SSR/engine bridge paths |
| Tests | `cargo test --workspace -j 2 -- --test-threads=1` (add `--lib` for a fast loop) | always |
| Version guard | `cargo test -p ferris-wasm --test wit_version` | always, and on every release |
| Module contract | `cargo test -p ferris-api --test architecture` (Core) and `cargo test -p ferris-server --lib router::tests::modules` | if you touched the module seam |
| Vertical HTTP | `cargo test -p ferris-print --features http` and `-p ferris-report --features http` | if you touched a vertical adapter |
| Style | `cd ferris-web && node_modules/.bin/tailwindcss -i style/tailwind.css -o style/desk.css --minify` | if classes or `tailwind.css` changed |
| Diff review | `git diff` | always |

Notes:

- The SSR *binary* is not part of `--all-targets`; it needs `ssr,wasm-runtime`.
- The test suite runs with `-j 2 -- --test-threads=1` because full parallelism
  exceeds the available memory. A memory limit, not a flaky workaround.
- `ferris-api` lives in Core: run its architecture test in the Core checkout
  whenever the module seam changes, before the Framework PR.
- If a gate cannot be run, say so and report it as **unverified**.

---

## 5. Repository hygiene and branches

- Branch layout (same in all three repositories):

| Branch | Role |
|---|---|
| `develop` | **Default.** Integration; every working PR targets it. |
| `master` | Production. Only `release/*` and `hotfix/*` merges land here. |
| `feature/*`, `fix/*` | Daily work, branched from `develop`. |
| `release/*` | Freeze before publishing; merged into `master` and back into `develop`. |
| `hotfix/*` | Urgent patch from `master`; merged into `master` and `develop`. |

- Protected branches: direct pushes to `master` and `develop` are disabled; a
  PR and the `CI` workflow are required. The required checks are `Format`,
  `Check`, `Wasm (hydrate + ssr)`, `Test` and `Tailwind`.
- Commit subjects: `feat(scope): …`, `fix(scope): …`, `refactor(…): …`,
  `docs(…): …`, `wip: …`. The body states what changed and why.
- `Cargo.lock` is **tracked** and updated by `cargo`, never by hand.
- Never commit `target/`, `ferris-web/pkg/`, `node_modules/`, `plugins/`,
  `ferris-files/tus_uploads/` or `test.db`.
- `vendor/serde`, `vendor/serde_core` are `[patch.crates-io]` overrides: keep
  them.
- A local `test/` scaffold (the `ferris new` WIT guest) is git-ignored, excluded
  from the workspace, and its `test/wit/ferris.wit` must stay byte-identical to
  `ferris-wasm/wit/ferris.wit` (`cmp`). Do not wire it into the workspace.
- Do not run a formatter over the whole repository as a side effect of an
  unrelated change.

---

## 6. Release

The version is one number shared by both workspaces, the WIT contract and
`FerrisDocs/project.json`. `VERSIONING.md` owns the full policy. While `0.x` the
project is pre-1.0; patch = no WIT change, minor = compatible additions, major =
breaking.

1. Branch `release/<version>` from `develop`.
2. Bump the version in **both** `Cargo.toml` files, both `ferris.wit` copies,
   `FerrisDocs/project.json` and `CHANGELOG.md`.
3. Rebuild the WIT fixture (`cargo build --target wasm32-wasip2 --manifest-path test/Cargo.toml`,
   when the local scaffold exists) and the three probes in
   `ferris-wasm/tests/fixtures/`.
4. Run every gate in §4 with `develop` checked out and CI green.
5. PR `release/<version>` → `master`; merge with a merge commit.
6. Tag `v<version>` on `master`, push the tag; the Docs workflow republishes the
   site from the new `project.json`.
7. Merge `master` back into `develop`.

A change that touches the Core contract (error taxonomy, `Module`, WIT-consumed
types) ships **Core first** and then the Framework bump.

---

## 7. Verification discipline

1. **Locate by grep, confirm by reading.** Every `file:line` was actually read.
2. **Negatives are proven by absence** with a `grep -rn` across all crates and
   the result quoted.
3. **Report what was not verified.**
4. **Do not invent APIs.** Read the trait, impl and call site first.
5. Note what was already correct and must not be disturbed.

Final report: what changed, which files, the exact commands run and their
result, and anything left unverified or knowingly skipped.

---

## 8. Documentation map

| File | What it is |
|---|---|
| `AGENTS.md` | This contract. |
| `README.md` | Public entry point for the product. |
| `VERSIONING.md` | One-version policy and release checklist. |
| `CHANGELOG.md` | Notable changes, `Keep a Changelog` format. |
| `../FerrisCore/AGENTS.md` | Contract for the base workspace. |
| `../FerrisDocs/` | Public documentation site (Astro + Starlight, Spanish prose). |

---

## 9. One-page checklist

- [ ] Everything ran inside the dev container (or was read-only on the host).
- [ ] Sanity check printed `1.98.1`; the sibling `../FerrisCore` is present.
- [ ] `cargo fmt --all -- --check` clean.
- [ ] `cargo clippy --all-targets` adds no new warnings; `cargo check-cli` if the
      heavy crates were touched.
- [ ] `cargo check --all-targets` passes; hydrate/SSR combos checked if relevant.
- [ ] Tests pass with `-j 2 -- --test-threads=1`, plus a regression test for any
      bug fixed.
- [ ] WIT fixture rebuilt and both `ferris.wit` copies identical if the contract
      changed; `wit_version` green when the version moved.
- [ ] Front end: reused global components; `Button`/`variants!`; only semantic
      tokens; `desk.css` rebuilt; no hardcoded doctypes; data via `DocStore`.
- [ ] No new `unwrap`/`expect`/panic on request paths; errors go through the
      `ferris-core` taxonomy.
- [ ] No `unsafe`, no new dependency without justification.
- [ ] No file grew past 600 lines (or the change states why not).
- [ ] The matching `FerrisDocs` page was updated for any public change.
- [ ] `git diff` reread; no `cargo clean`, no host artifacts.
- [ ] If context was compacted: the summary keeps the task, the context, the
      rules and the next step.
