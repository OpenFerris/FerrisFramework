//! One version number for the framework and its app contract.
//!
//! `Cargo.toml` and the WIT package declare the same version, and that number
//! is the compatibility promise: a patch says nothing in the contract changed,
//! a minor adds backwards-compatible surface, and a major breaks it. Keeping
//! them in sync by hand is exactly the kind of invariant that drifts silently,
//! so these tests fail as soon as the two files disagree.
//!
//! `test/wit/ferris.wit` is the fixture the scaffold ships; it must be the same
//! bytes as the contract the host compiles against.

// The integration test inherits the package's dependency list; this one only
// reads two files from disk.
#![allow(unused_crate_dependencies)]

use std::fs;
use std::path::{Path, PathBuf};

/// The contract wave this checkout ships. While Ferris is pre-1.0 every minor is
/// a distinct contract (a host and a guest on different `0.x` minors do not
/// link), so the wave is pinned here explicitly instead of being derived from
/// the crate version: drifting WIT copies fail with the version they claim, not
/// with a number the build happens to carry.
const WIT_VERSION: &str = "0.4.0";

fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `<repo>/ferris-wasm`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("ferris-wasm lives inside the repository")
        .to_path_buf()
}

fn contract() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("wit/ferris.wit")
}

/// The `package …;` line of a contract source. It follows the banner comment, so
/// the file does not literally start with it.
fn declared_package(source: &str) -> Option<&str> {
    source
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("package ferris:framework@"))
}

#[test]
fn wit_package_version_is_the_contract_wave() {
    let source = fs::read_to_string(contract()).expect("read wit/ferris.wit");
    let expected = format!("package ferris:framework@{WIT_VERSION};");

    assert_eq!(
        declared_package(&source),
        Some(expected.as_str()),
        "the contract must declare the 0.4.0 wave; bump `WIT_VERSION` together \
         with both WIT copies and the workspace version"
    );
}

#[test]
fn wit_version_matches_the_workspace_version() {
    // `CARGO_PKG_VERSION` is `[workspace.package] version`, which every crate and
    // the WIT package share (VERSIONING.md). A change that moves one of them must
    // move all three in the same commit: `Cargo.toml`, `ferris-wasm/wit/ferris.wit`
    // and `test/wit/ferris.wit`.
    assert_eq!(
        WIT_VERSION,
        env!("CARGO_PKG_VERSION"),
        "the workspace version must equal the contract wave (`{WIT_VERSION}`); \
         a release bumps `[workspace.package] version` in the root Cargo.toml in \
         the same change as the two WIT copies"
    );
}

#[test]
fn the_scaffold_fixture_is_byte_identical_when_present() {
    let fixture_path = repo_root().join("test/wit/ferris.wit");
    // `test/` is git-ignored (it is the generated `ferris new` scaffold), so a
    // clean clone does not carry it. When it exists, it must match exactly.
    if !fixture_path.exists() {
        eprintln!(
            "skipping: {} is absent (the scaffold is not part of the repository)",
            fixture_path.display()
        );
        return;
    }

    let contract = fs::read(contract()).expect("read ferris-wasm/wit/ferris.wit");
    let fixture = fs::read(&fixture_path).expect("read test/wit/ferris.wit");

    assert_eq!(
        contract,
        fixture,
        "{} must stay byte-identical to ferris-wasm/wit/ferris.wit",
        fixture_path.display()
    );
}
