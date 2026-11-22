//! Embedded widget-preview images (`static/previews/`, D3 of
//! the dashboard widget).
//!
//! The Desk's widget catalog draws a preview `<img>` per card from
//! `/static/previews/*`: each `WidgetDef::preview` URL points here, and the
//! ones the host itself ships (`ferris-core::host_widgets`) are the three
//! built-in kinds plus the generic fallback. The server answers those
//! requests from this table rather than from the filesystem — the same
//! deployment story as [`crate::vendor_assets`] and [`crate::DESK_CSS`]:
//! bytes compiled into `ferris-server` and `ferris-web-ssr`, no directory
//! layout next to the binary.
//!
//! The Desk does not merely `<img>` these files: [`crate::views::dashboard`]
//! fetches each one, sanitizes it and inlines it in a shadow root, where the
//! page's `var(--token)` reach it — so a preview follows the active theme. An
//! `<img src="*.svg">` loaded on its own *cannot* (an `<img>` SVG is an
//! isolated document), which is why the run-time path is the inline one; the
//! `<img>` is only the SSR/first-hydrate fallback. These shipped SVGs therefore
//! paint with `var(--token, <literal>)` and carry a `@media
//! (prefers-color-scheme: dark)` block for the fallback case. Gated on
//! `not(wasm32)` for the same reason as `vendor_assets`: the browser fetches
//! them over HTTP, the bytes must never enter the wasm bundle. The tests below
//! are what keeps the table honest.

/// Every preview file the wire serves, keyed by its path under
/// `/static/previews/` (no leading slash, no `..` — the comparison in
/// [`lookup`] is exact, so a traversal cannot match anything).
static FILES: &[(&str, &[u8])] = &[
    ("chart.svg", include_bytes!("../static/previews/chart.svg")),
    (
        "generic.svg",
        include_bytes!("../static/previews/generic.svg"),
    ),
    ("stat.svg", include_bytes!("../static/previews/stat.svg")),
    ("table.svg", include_bytes!("../static/previews/table.svg")),
];

/// Resolve a `/static/previews/{*path}` wildcard capture (`path` is the part
/// after the prefix) to its `Content-Type` and bytes.
///
/// Returns `None` for anything not committed — the handler answers 404
/// without touching the disk.
pub fn lookup(path: &str) -> Option<(&'static str, &'static [u8])> {
    FILES
        .iter()
        .find(|(name, _)| *name == path)
        .map(|(name, bytes)| (content_type(name), *bytes))
}

/// MIME type by extension; previews are SVG today, PNG/WebP are named so a
/// future asset only has to land on disk and in the table.
fn content_type(path: &str) -> &'static str {
    if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".webp") {
        "image/webp"
    } else {
        "image/svg+xml"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    /// Every file committed under `static/previews/` must be in the table —
    /// a preview nobody serves is a broken `<img>` (and a silent `on:error`
    /// fallback) waiting to happen.
    #[test]
    fn every_committed_preview_file_is_embedded() {
        let mut on_disk = Vec::new();
        collect_files(Path::new("static/previews"), &mut on_disk);
        assert!(
            !on_disk.is_empty(),
            "static/previews/ is empty — the catalog cards have nothing to show"
        );

        let embedded: HashSet<&str> = FILES.iter().map(|(name, _)| *name).collect();
        let mut missing: Vec<String> = on_disk
            .iter()
            .filter(|path| !embedded.contains(path.as_str()))
            .cloned()
            .collect();
        missing.sort();
        assert!(
            missing.is_empty(),
            "committed preview files missing from preview_assets::FILES: {missing:?}"
        );
    }

    /// The inverse: nothing is served that is not on disk (a stale row would
    /// 500 at `include_bytes!` compile time, but a renamed file on disk would
    /// 404 at runtime — this catches both directions).
    #[test]
    fn no_embedded_file_is_missing_from_disk() {
        let mut files = Vec::new();
        collect_files(Path::new("static/previews"), &mut files);
        let on_disk: HashSet<String> = files.into_iter().collect();
        let stale: Vec<&&str> = FILES
            .iter()
            .map(|(name, _)| name)
            .filter(|name| !on_disk.contains(**name))
            .collect();
        assert!(stale.is_empty(), "embedded but missing on disk: {stale:?}");
    }

    /// Table hygiene: unique keys, relative paths, non-empty bytes.
    #[test]
    fn table_keys_are_unique_and_relative() {
        let mut seen = HashSet::new();
        for (name, bytes) in FILES {
            assert!(!name.starts_with('/'), "absolute path in table: {name}");
            assert!(!name.contains(".."), "relative traversal in table: {name}");
            assert!(!bytes.is_empty(), "empty preview file: {name}");
            assert!(seen.insert(name), "duplicate key in table: {name}");
        }
    }

    /// The four fallbacks the Desk's kind-based default and `on:error` path
    /// request must all resolve with an image content type.
    #[test]
    fn lookup_serves_every_fallback_preview() {
        for name in ["stat.svg", "chart.svg", "table.svg", "generic.svg"] {
            let (content_type, bytes) = lookup(name).unwrap_or_else(|| panic!("{name} embedded"));
            assert_eq!(content_type, "image/svg+xml");
            assert!(
                bytes.len() > 200,
                "{name} looks truncated ({} bytes)",
                bytes.len()
            );
        }
    }

    #[test]
    fn lookup_rejects_anything_not_committed() {
        assert!(lookup("nope.svg").is_none());
        assert!(lookup("").is_none());
        assert!(lookup("../Cargo.toml").is_none());
        assert!(lookup("/stat.svg").is_none());
    }

    /// Walk a preview directory collecting every path relative to it.
    fn collect_files(dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            panic!("cannot read {}", dir.display());
        };
        for entry in entries {
            let entry = entry.expect("read_dir entry");
            let path: PathBuf = entry.path();
            if path.is_dir() {
                collect_files(&path, out);
            } else {
                let rel = path
                    .strip_prefix(dir)
                    .expect("path under the preview dir")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
    }
}
