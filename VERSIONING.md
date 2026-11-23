# Versioning and compatibility

Ferris has **one version number**. The workspace and the app contract read it
from the same place:

| Where | Declared as |
|---|---|
| `Cargo.toml` | `[workspace.package] version = "0.4.0"` |
| `ferris-wasm/wit/ferris.wit` | `package ferris:framework@0.4.0;` |
| `test/wit/ferris.wit` | byte-identical copy of the contract |

The number is a compatibility promise, not a count of changes:

| Bump | Meaning | Effect on an installed app |
|---|---|---|
| **patch** `x.y.Z` | fixes, performance, docs, internal refactors | none; the WIT does not change |
| **minor** `x.Y.z` | backwards-compatible WIT additions | keeps instantiating; may use the new surface |
| **major** `X.y.z` | breaking WIT or public API change | must be recompiled and adapted |

`1.0.0` is the first stable release. Until then Ferris is **pre-1.0**: a `0.x`
minor release may break compatibility, so pin a tag or commit for anything
long-lived. From `1.0.0` on, the table above is enforced.

The same number is used by `cargo`, the CLI (`ferris --version`), the OpenAPI
document and the WIT package; there is no separate contract version.

## Guard

`ferris-wasm/tests/wit_version.rs` fails when:

- the `package ferris:framework@…;` line differs from `CARGO_PKG_VERSION`, or
- `test/wit/ferris.wit` is not byte-identical to `ferris-wasm/wit/ferris.wit`.

Any release therefore updates `Cargo.toml` and both WIT copies in the same
change, and `cargo test -p ferris-wasm` proves it.

## Releasing

1. Bump `Cargo.toml` and both WIT copies to the same version.
2. Rebuild the WIT fixtures (`test/Cargo.toml` and the three
   `ferris-wasm/tests/fixtures/*` probes).
3. Run the gates in `AGENTS.md` §3.
4. Add the entry to `CHANGELOG.md`, stating the kind of bump.
5. Archive the previous documentation version (`starlight-versions`); the site
   keeps the current docs at `/` and released copies at `/<version>/`.

The user-facing explanation, including the exact list of breaking and
non-breaking changes, lives in
[`docs-site/src/content/docs/referencia/versionado.md`](docs-site/src/content/docs/referencia/versionado.md).
