//! Typst resources, font loading and the per-render Typst world.

use chrono::{Datelike, Timelike, Utc};
use ferris_core::Result;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Feature, Features, Library, World};

/// Shared, immutable Typst resources (library, font book, and font faces).
///
/// These are loaded once and reused across every render.
pub(super) struct TypstResources {
    library: LazyHash<Library>,
    pub(super) book: LazyHash<FontBook>,
    pub(super) fonts: Vec<Font>,
}

impl TypstResources {
    pub(super) fn new(font_dir: Option<&Path>) -> Result<Self> {
        let features = std::iter::once(Feature::Html).collect::<Features>();
        let library = Library::builder().with_features(features).build();

        let mut fonts = Vec::new();
        for data in typst_assets::fonts() {
            if let Some(font) = Font::new(Bytes::new(data), 0) {
                fonts.push(font);
            }
        }

        if let Some(dir) = font_dir {
            Self::load_font_dir(&mut fonts, dir)?;
        }

        let book = FontBook::from_fonts(&fonts);
        Ok(Self {
            library: LazyHash::new(library),
            book: LazyHash::new(book),
            fonts,
        })
    }

    /// Load every TrueType/OpenType font found in a directory.
    fn load_font_dir(fonts: &mut Vec<Font>, dir: &Path) -> Result<()> {
        let entries = std::fs::read_dir(dir).map_err(|e| {
            ferris_core::FileError::Storage(format!(
                "cannot read font directory {}: {e}",
                dir.display()
            ))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                ferris_core::FileError::Storage(format!(
                    "cannot read font directory {}: {e}",
                    dir.display()
                ))
            })?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let supported = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| matches!(ext, "ttf" | "otf" | "ttc"));
            if !supported.unwrap_or(false) {
                continue;
            }

            let data = std::fs::read(&path).map_err(|e| {
                ferris_core::FileError::Storage(format!(
                    "cannot read font file {}: {e}",
                    path.display()
                ))
            })?;
            let data = Bytes::new(data);

            // Font collections can contain multiple faces.
            for index in 0..4u32 {
                match Font::new(data.clone(), index) {
                    Some(font) => fonts.push(font),
                    None => break,
                }
            }
        }

        Ok(())
    }
}

/// Typst world for a single document render.
///
/// The main source is synthesized from the user template: a wrapper reads a
/// virtual `data.json` and binds every top-level key of the data object as a
/// Typst variable, so templates can reference `#customer`, `#items`, etc.
pub(super) struct TypstWorld {
    resources: Arc<TypstResources>,
    main_file: FileId,
    source: Source,
    data_files: HashMap<PathBuf, Bytes>,
    template_dir: Option<PathBuf>,
}

impl TypstWorld {
    pub(super) fn new(
        resources: Arc<TypstResources>,
        template_dir: Option<PathBuf>,
        template: &str,
        data: &ferris_core::Value,
        data_bytes: Bytes,
        prelude: &str,
    ) -> Self {
        let main_file = FileId::new(None, VirtualPath::new("main.typ"));
        let source = Source::new(main_file, Self::synthesize_source(template, data, prelude));

        let mut data_files = HashMap::new();
        data_files.insert(PathBuf::from("data.json"), data_bytes);

        Self {
            resources,
            main_file,
            source,
            data_files,
            template_dir,
        }
    }

    /// Build the main source that injects the JSON document as Typst variables.
    pub(super) fn synthesize_source(
        template: &str,
        data: &ferris_core::Value,
        prelude: &str,
    ) -> String {
        let mut src = String::with_capacity(template.len() + prelude.len() + 256);
        // The page prelude goes **before** the per-field bindings: a document
        // with a field named `page`, `text` or `scale` would otherwise shadow
        // the Typst function the prelude calls.
        if !prelude.is_empty() {
            src.push_str(prelude);
            src.push('\n');
        }
        src.push_str("#let _ferris_data = json.decode(read(\"data.json\", encoding: \"utf8\"))\n");

        if let Some(obj) = data.as_object() {
            let mut keys: Vec<&String> = obj.keys().collect();
            keys.sort();
            for key in keys {
                if Self::is_valid_identifier(key) {
                    let _ = writeln!(src, "#let {key} = _ferris_data.at(\"{key}\")");
                }
            }
        }

        src.push('\n');
        src.push_str(template);
        src
    }

    /// Typst identifiers allow alphanumerics, `_`, and `-`, but must start with
    /// a letter or `_`.
    pub(super) fn is_valid_identifier(key: &str) -> bool {
        let mut chars = key.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
            _ => return false,
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }
}

impl World for TypstWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.resources.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.resources.book
    }

    fn main(&self) -> FileId {
        self.main_file
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main_file {
            Ok(self.source.clone())
        } else {
            Err(FileError::NotFound(
                id.vpath().as_rooted_path().to_path_buf(),
            ))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        let path = id.vpath().as_rootless_path().to_path_buf();

        if let Some(bytes) = self.data_files.get(&path) {
            return Ok(bytes.clone());
        }

        // Only serve files inside the configured template directory (with
        // traversal protection): reject `../`, absolute paths, and symlinks
        // escaping the root.
        let Some(dir) = &self.template_dir else {
            return Err(FileError::NotFound(path));
        };
        let base = dir
            .canonicalize()
            .map_err(|_| FileError::NotFound(path.clone()))?;
        let candidate = base.join(&path);
        let resolved = candidate
            .canonicalize()
            .map_err(|_| FileError::NotFound(path.clone()))?;
        if !resolved.starts_with(&base) {
            return Err(FileError::NotFound(path));
        }
        let bytes = std::fs::read(&resolved).map_err(|_| FileError::NotFound(path))?;
        Ok(Bytes::new(bytes))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.resources.fonts.get(index).cloned()
    }

    // The `as u8` casts are safe: chrono components are in `u8` range.
    #[allow(clippy::cast_possible_truncation)]
    fn today(&self, offset: Option<i64>) -> Option<Datetime> {
        let now = Utc::now() + chrono::Duration::seconds(offset.unwrap_or(0));
        Datetime::from_ymd_hms(
            now.year(),
            now.month() as u8,
            now.day() as u8,
            now.hour() as u8,
            now.minute() as u8,
            now.second() as u8,
        )
    }
}
