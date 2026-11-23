#![allow(unused_crate_dependencies)]

//! Every literal `t("…")` in the Desk must exist in both shipped catalogues.
//!
//! The UI falls back to the key when a message is missing, which is useful in
//! development but hides a half-translated screen in production. This test scans
//! the two front-end crates (`ferris-web` and its sibling `ferris-web-ui`) for
//! literal keys and compares them with the merged `en`/`es` catalogues. A key
//! added to one language only fails here as well, because the same set is
//! checked against both.
//!
//! Dynamic keys (`t(&key)` from a table) are out of scope: they cannot be
//! enumerated, and their tables are covered by the components' own tests.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const PREFIXES: &[&str] = &[
    "login.",
    "common.",
    "nav.",
    "error.",
    "views.",
    "desk.",
    "components.",
    "ui.",
];

fn catalog_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../FerrisCore/ferris-core/src/i18n")
}

fn ui_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../FerrisCore/ferris-web-ui/src")
}

/// Parse the flat/quoted and the `[table]` forms the catalogues use. Values are
/// always single-line strings, so the key is what matters.
fn catalog_keys(path: &Path, keys: &mut BTreeSet<String>) {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("reading {} failed: {error}", path.display()));
    let mut prefix = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            prefix = format!("{}.", line.trim_matches(['[', ']']).trim());
            continue;
        }
        if let Some((key, _)) = line.split_once('=') {
            let key = key.trim().trim_matches('"');
            if !key.is_empty() {
                keys.insert(format!("{prefix}{key}"));
            }
        }
    }
}

fn add_catalog(dir: &Path, locale: &str, keys: &mut BTreeSet<String>) {
    for file in [
        format!("{locale}.toml"),
        format!("{locale}.views.toml"),
        format!("{locale}.desk.toml"),
        format!("{locale}.components.toml"),
        format!("{locale}.ui.toml"),
    ] {
        let path = dir.join(file);
        assert!(path.is_file(), "missing catalogue {}", path.display());
        catalog_keys(&path, keys);
    }
}

/// Collect `t("key")`, `t_with("key")`, `.t("key")` and `.t_with("key")` from
/// lines that are code (comments and doc comments are skipped).
fn add_used(dir: &Path, used: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            add_used(&path, used);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {} failed: {error}", path.display()));
        let mut in_tests = false;
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.contains("mod tests") {
                in_tests = true;
            }
            // Test modules use their own fixtures; production keys are what the
            // shipped screens render.
            if in_tests || trimmed.starts_with("//") {
                continue;
            }
            for needle in ["t(\"", "t_with(\"", ".t(\"", ".t_with(\""] {
                let mut rest = line;
                while let Some(index) = rest.find(needle) {
                    let after = &rest[index + needle.len()..];
                    let Some(end) = after.find('"') else { break };
                    let key = &after[..end];
                    if PREFIXES.iter().any(|prefix| key.starts_with(prefix)) {
                        used.insert(key.to_string());
                    }
                    rest = &after[end..];
                }
            }
        }
    }
}

#[test]
fn every_literal_desk_key_exists_in_both_catalogues() {
    let dir = catalog_dir();
    assert!(
        dir.is_dir(),
        "the sibling FerrisCore checkout is required at {}",
        dir.display()
    );
    let mut en = BTreeSet::new();
    let mut es = BTreeSet::new();
    add_catalog(&dir, "en", &mut en);
    add_catalog(&dir, "es", &mut es);

    let mut used = BTreeSet::new();
    add_used(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut used,
    );
    let ui = ui_src();
    if ui.is_dir() {
        add_used(&ui, &mut used);
    } else {
        eprintln!("SKIP: sibling UI sources not found at {}", ui.display());
    }

    let missing_english: Vec<&String> = used.difference(&en).collect();
    let missing_spanish: Vec<&String> = used.difference(&es).collect();
    assert!(
        missing_english.is_empty() && missing_spanish.is_empty(),
        "keys used in the Desk but missing from the catalogues:\n  en: {missing_english:?}\n  es: {missing_spanish:?}"
    );
}
