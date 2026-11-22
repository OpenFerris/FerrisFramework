//! Embedded copies of the vendored browser libraries (`static/vendor/`).
//!
//! The Desk loads Leaflet, JsBarcode, Quill and marked lazily from
//! `/static/vendor/*` (see [`crate::ext::vendor`]); the server answers those
//! requests from this table rather than from the filesystem. The bytes are
//! compiled into `ferris-server` and `ferris-web-ssr` exactly like
//! [`crate::DESK_CSS`], so a deployment needs no `node_modules` and no
//! directory layout next to the binary.
//!
//! The files themselves are generated (`npm run vendor` — see
//! `scripts/vendor.mjs`) and committed; this table decides which of them the
//! wire serves. Gated on `not(wasm32)` because the browser fetches them
//! over HTTP: the bytes must never be linked into the wasm bundle, the same
//! reason `DESK_CSS` is gated on `ssr`.
//!
//! The table is a data table — one row per committed file — which is the
//! written exception to the 600-line rule for hand-written sources
//! (`AGENTS.md` §4). The tests below are what keeps it honest.

/// Every vendor file the wire serves, keyed by its path under
/// `/static/vendor/` (no leading slash, no `..` — the comparison in
/// [`lookup`] is exact, so a traversal cannot match anything).
static FILES: &[(&str, &[u8])] = &[
    (
        "jsbarcode/JsBarcode.all.min.js",
        include_bytes!("../static/vendor/jsbarcode/JsBarcode.all.min.js"),
    ),
    (
        "jsbarcode/LICENSE",
        include_bytes!("../static/vendor/jsbarcode/LICENSE"),
    ),
    (
        "leaflet-draw/images/layers-2x.png",
        include_bytes!("../static/vendor/leaflet-draw/images/layers-2x.png"),
    ),
    (
        "leaflet-draw/images/layers.png",
        include_bytes!("../static/vendor/leaflet-draw/images/layers.png"),
    ),
    (
        "leaflet-draw/images/marker-icon-2x.png",
        include_bytes!("../static/vendor/leaflet-draw/images/marker-icon-2x.png"),
    ),
    (
        "leaflet-draw/images/marker-icon.png",
        include_bytes!("../static/vendor/leaflet-draw/images/marker-icon.png"),
    ),
    (
        "leaflet-draw/images/marker-shadow.png",
        include_bytes!("../static/vendor/leaflet-draw/images/marker-shadow.png"),
    ),
    (
        "leaflet-draw/images/spritesheet-2x.png",
        include_bytes!("../static/vendor/leaflet-draw/images/spritesheet-2x.png"),
    ),
    (
        "leaflet-draw/images/spritesheet.png",
        include_bytes!("../static/vendor/leaflet-draw/images/spritesheet.png"),
    ),
    (
        "leaflet-draw/images/spritesheet.svg",
        include_bytes!("../static/vendor/leaflet-draw/images/spritesheet.svg"),
    ),
    (
        "leaflet-draw/leaflet.draw.css",
        include_bytes!("../static/vendor/leaflet-draw/leaflet.draw.css"),
    ),
    (
        "leaflet-draw/leaflet.draw.js",
        include_bytes!("../static/vendor/leaflet-draw/leaflet.draw.js"),
    ),
    (
        "leaflet/LICENSE",
        include_bytes!("../static/vendor/leaflet/LICENSE"),
    ),
    (
        "leaflet/images/layers-2x.png",
        include_bytes!("../static/vendor/leaflet/images/layers-2x.png"),
    ),
    (
        "leaflet/images/layers.png",
        include_bytes!("../static/vendor/leaflet/images/layers.png"),
    ),
    (
        "leaflet/images/marker-icon-2x.png",
        include_bytes!("../static/vendor/leaflet/images/marker-icon-2x.png"),
    ),
    (
        "leaflet/images/marker-icon.png",
        include_bytes!("../static/vendor/leaflet/images/marker-icon.png"),
    ),
    (
        "leaflet/images/marker-shadow.png",
        include_bytes!("../static/vendor/leaflet/images/marker-shadow.png"),
    ),
    (
        "leaflet/leaflet.css",
        include_bytes!("../static/vendor/leaflet/leaflet.css"),
    ),
    (
        "leaflet/leaflet.js",
        include_bytes!("../static/vendor/leaflet/leaflet.js"),
    ),
    (
        "marked/LICENSE",
        include_bytes!("../static/vendor/marked/LICENSE"),
    ),
    (
        "marked/marked.min.js",
        include_bytes!("../static/vendor/marked/marked.min.js"),
    ),
    (
        "quill/LICENSE",
        include_bytes!("../static/vendor/quill/LICENSE"),
    ),
    (
        "quill/quill.core.css",
        include_bytes!("../static/vendor/quill/quill.core.css"),
    ),
    (
        "quill/quill.js",
        include_bytes!("../static/vendor/quill/quill.js"),
    ),
    (
        "quill/quill.snow.css",
        include_bytes!("../static/vendor/quill/quill.snow.css"),
    ),
];

/// Resolve a `/static/vendor/{*path}` wildcard capture (`path` is the part
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

/// MIME type by extension; anything without a known extension is the plain
/// text it is (the `LICENSE` files).
fn content_type(path: &str) -> &'static str {
    if path.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "text/plain; charset=utf-8"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    /// Every file committed under `static/vendor/` (except the README) must be
    /// in the table — a vendor file nobody serves is a silent 404 waiting to
    /// break a widget on the next `npm run vendor`.
    #[test]
    fn every_committed_vendor_file_is_embedded() {
        let mut on_disk = Vec::new();
        collect_files(Path::new("static/vendor"), &mut on_disk);
        assert!(
            !on_disk.is_empty(),
            "static/vendor/ is empty — npm run vendor?"
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
            "committed vendor files missing from vendor_assets::FILES: {missing:?}"
        );
    }

    /// The inverse: nothing is served that is not on disk (a stale row would
    /// 500 at `include_bytes!` compile time, but a renamed file on disk would
    /// 404 at runtime — this catches both directions).
    #[test]
    fn no_embedded_file_is_missing_from_disk() {
        let mut files = Vec::new();
        collect_files(Path::new("static/vendor"), &mut files);
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
            assert!(!bytes.is_empty(), "empty vendor file: {name}");
            assert!(seen.insert(name), "duplicate key in table: {name}");
        }
    }

    #[test]
    fn lookup_serves_known_files_with_a_content_type() {
        let (content_type, bytes) = lookup("leaflet/leaflet.js").expect("leaflet.js is committed");
        assert!(content_type.starts_with("application/javascript"));
        assert!(bytes.len() > 1000, "leaflet.js looks truncated");

        let (content_type, _) = lookup("leaflet/leaflet.css").expect("leaflet.css is committed");
        assert!(content_type.starts_with("text/css"));

        let (content_type, _) = lookup("leaflet/LICENSE").expect("licence is committed");
        assert!(content_type.starts_with("text/plain"));
    }

    #[test]
    fn lookup_rejects_anything_not_committed() {
        assert!(lookup("nope/nope.js").is_none());
        assert!(lookup("").is_none());
        assert!(lookup("../Cargo.toml").is_none());
        assert!(lookup("/leaflet/leaflet.js").is_none());
    }

    /// Walk `static/vendor/` collecting every path relative to it.
    fn collect_files(dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            panic!("cannot read {} — run `npm run vendor`", dir.display());
        };
        for entry in entries {
            let entry = entry.expect("read_dir entry");
            let path: PathBuf = entry.path();
            if path.is_dir() {
                collect_files(&path, out);
            } else {
                let rel = path
                    .strip_prefix("static/vendor")
                    .expect("path under static/vendor")
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel != "README.md" {
                    out.push(rel);
                }
            }
        }
    }
}
