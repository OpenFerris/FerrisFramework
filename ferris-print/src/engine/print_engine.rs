//! The [`PrintEngine`]: format registry and render entry points.
//!
//! The engine is pure and synchronous: [`PrintEngine::render_format`] turns a
//! format plus a document into bytes and touches neither the database nor the
//! network. The `http` feature is the only place that reads rows, checks
//! permissions and offloads the call to a blocking task.

use ferris_core::{DocumentError, Error, Result, SerializationError, ValidationError, Value};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info};
use typst::foundations::Bytes;

use super::config::{PrintConfig, PrintFormat, PrintFormatDef, PrintOptions};
use super::export::export;
use super::options::ResolvedPrint;
use super::template;
use super::world::{TypstResources, TypstWorld};

/// Print engine using Typst.
pub struct PrintEngine {
    config: PrintConfig,
    formats: Arc<RwLock<HashMap<String, PrintFormatDef>>>,
    resources: Arc<TypstResources>,
}

impl PrintEngine {
    /// Build the engine, loading the bundled fonts and every extra font in
    /// `config.font_dir` once.
    ///
    /// # Errors
    ///
    /// A storage error when the font directory exists but cannot be read.
    pub fn new(config: PrintConfig) -> Result<Self> {
        if let Some(dir) = config
            .template_dir
            .as_deref()
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
        {
            let path = Path::new(dir);
            let metadata = std::fs::metadata(path).map_err(|error| {
                Error::Validation(ValidationError::Invalid(format!(
                    "print template root `{dir}` is not readable: {error}"
                )))
            })?;
            if !metadata.is_dir() {
                return Err(Error::Validation(ValidationError::Invalid(format!(
                    "print template root `{dir}` is not a directory"
                ))));
            }
        }
        let resources = Arc::new(TypstResources::new(
            config.font_dir.as_deref().map(Path::new),
        )?);

        Ok(Self {
            config,
            formats: Arc::new(RwLock::new(HashMap::new())),
            resources,
        })
    }

    #[must_use]
    pub const fn config(&self) -> &PrintConfig {
        &self.config
    }

    /// Register a print format.
    pub fn register_format(&self, format: PrintFormatDef) {
        self.formats.write().insert(format.name.clone(), format);
    }

    #[must_use]
    pub fn get_format(&self, name: &str) -> Option<PrintFormatDef> {
        self.formats.read().get(name).cloned()
    }

    #[must_use]
    pub fn list_formats(&self) -> Vec<PrintFormatDef> {
        self.formats.read().values().cloned().collect()
    }

    /// Render a registered format.
    ///
    /// Looks the format up in the registry and delegates to
    /// [`Self::render_format`], so the registered and ad-hoc (database/builtin)
    /// paths apply the same option handling.
    ///
    /// # Errors
    ///
    /// The format must exist and be enabled; every unsupported option is a
    /// typed error (see [`Self::render_format`]).
    pub fn render(
        &self,
        format_name: &str,
        data: &Value,
        options: &PrintOptions,
    ) -> Result<Vec<u8>> {
        let format = self
            .formats
            .read()
            .get(format_name)
            .cloned()
            .ok_or_else(|| {
                Error::Document(DocumentError::NotFound {
                    doctype: "Print Format".to_string(),
                    name: format_name.to_string(),
                })
            })?;
        if !format.enabled {
            return Err(Error::Validation(ValidationError::Invalid(format!(
                "Print format {format_name} is disabled"
            ))));
        }
        self.render_format(&format, data, options)
    }

    /// Render the Typst source of `format`, without compiling it.
    ///
    /// Useful for a preview of the generated layout and for tests that need to
    /// assert what `MiniJinja` produced. A `Typst` format returns its template
    /// unchanged; a `Jinja` format is rendered with the same context and limits
    /// the compiler path uses.
    ///
    /// # Errors
    ///
    /// A validation error when the template is over the size limit or fails to
    /// render.
    pub fn render_source(
        &self,
        format: &PrintFormatDef,
        data: &Value,
        options: &PrintOptions,
    ) -> Result<String> {
        template::render_source(format, data, options, &self.config)
    }

    /// Render an in-memory format without registering it.
    ///
    /// This is the path the API uses for a `Print Format` row (or the builtin
    /// fallback): the template source is passed in, so a database edit is
    /// honoured on the next request instead of requiring a registry reload.
    ///
    /// `paper_size`, `margins`, `orientation` and `scale` are applied as a
    /// Typst page prelude; `watermark` becomes the page background. Anything
    /// the renderer cannot honour is a typed error rather than a silent no-op.
    /// `scale` is bounded (`0.1..=4.0`) and a PNG whose raster would exceed the
    /// pixel ceiling is refused here, before Typst allocates it.
    ///
    /// This call is synchronous and CPU-bound and has **no internal deadline**:
    /// a request path must run it through `http::render::render_with_limits`
    /// (slot semaphore + wall-clock budget), never directly on a runtime thread.
    ///
    /// # Errors
    ///
    /// A validation error for an unusable option or a template that fails to
    /// render, or the Typst diagnostic when compilation/export fails.
    pub fn render_format(
        &self,
        format: &PrintFormatDef,
        data: &Value,
        options: &PrintOptions,
    ) -> Result<Vec<u8>> {
        let resolved = ResolvedPrint::resolve(format, options)?;
        let source = template::render_source(format, data, options, &self.config)?;
        let world = self.build_world(format, &source, data, &resolved)?;
        debug!(
            format = %format.name,
            engine = ?format.engine,
            output_format = ?resolved.format,
            "rendering print document"
        );
        info!(format = %format.name, "rendering print document");
        export(&world, resolved.format, resolved.scale)
    }

    /// Render to HTML.
    ///
    /// # Errors
    ///
    /// The format must be registered/enabled and render successfully; the
    /// output must be valid UTF-8.
    pub fn render_html(
        &self,
        format_name: &str,
        data: &Value,
        options: PrintOptions,
    ) -> Result<String> {
        let mut options = options;
        options.format = PrintFormat::Html;
        let bytes = self.render(format_name, data, &options)?;
        String::from_utf8(bytes).map_err(|e| {
            ferris_core::Error::from(ferris_core::FileError::Storage(format!(
                "HTML output is not valid UTF-8: {e}"
            )))
        })
    }

    /// Render to PNG.
    ///
    /// # Errors
    ///
    /// The format must be registered/enabled and render successfully.
    pub fn render_image(
        &self,
        format_name: &str,
        data: &Value,
        options: PrintOptions,
    ) -> Result<Vec<u8>> {
        let mut options = options;
        options.format = PrintFormat::Png;
        self.render(format_name, data, &options)
    }

    /// Build a Typst world for a single render of a format definition.
    fn build_world(
        &self,
        format: &PrintFormatDef,
        source: &str,
        data: &Value,
        resolved: &ResolvedPrint,
    ) -> Result<TypstWorld> {
        let json = serde_json::to_vec(data)
            .map_err(|e| Error::Serialization(SerializationError::Json(e)))?;
        let data_bytes = Bytes::new(json);
        // The same directory the Jinja loader uses, so an include and a Typst
        // `read()` share one boundary.
        let template_dir = template::resolve_dir(&self.config, format)?;
        // A page configuration is meaningless in the HTML target, and Typst
        // rejects it there ("page configuration is not allowed inside of
        // containers"), which made `format: html` fail for every layout. The
        // page prelude is for the paged formats only.
        let prelude = if matches!(resolved.format, PrintFormat::Html) {
            String::new()
        } else {
            resolved.page_prelude()
        };
        Ok(TypstWorld::new(
            self.resources.clone(),
            template_dir,
            source,
            data,
            data_bytes,
            &prelude,
        ))
    }
}
