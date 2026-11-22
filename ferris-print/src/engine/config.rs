//! Print configuration and the metadata that describes a print format.
//!
//! A [`PrintFormatDef`] is the whole input of the renderer: the template
//! source, its engine, the parameters a user may override and the page
//! geometry. Everything else (database rows, permissions, HTTP) belongs to the
//! `http` feature; this module is pure data.

use chrono::{DateTime, Utc};
use ferris_core::{Error, Result, ValidationError, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Print configuration: the shared resources and the limits applied to every
/// user-authored template.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrintConfig {
    /// Directory used for `{% include %}`/`{% extends %}` in Jinja formats and
    /// for files a Typst template reads. `None` disables both.
    pub template_dir: Option<String>,
    /// Extra fonts loaded into every Typst render, in addition to the bundled
    /// set.
    pub font_dir: Option<String>,
    pub limits: TemplateLimits,
}

/// Bounds applied to a user-authored template.
///
/// A print format is editable from the Desk, so it is treated as untrusted
/// input even when the row was written by an administrator: a runaway loop or
/// a template that expands to hundreds of megabytes must fail as a validation
/// error, not stall the server.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TemplateLimits {
    pub max_template_bytes: usize,
    pub max_output_bytes: usize,
    /// `MiniJinja` fuel: roughly one unit per instruction and one per 64 bytes
    /// of data a template creates.
    pub max_fuel: u64,
    pub recursion_limit: usize,
}

impl Default for TemplateLimits {
    fn default() -> Self {
        Self {
            max_template_bytes: 256 * 1024,
            max_output_bytes: 4 * 1024 * 1024,
            max_fuel: 20_000_000,
            recursion_limit: 100,
        }
    }
}

/// Output format of a render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrintFormat {
    Pdf,
    Html,
    Png,
    Svg,
}

impl std::str::FromStr for PrintFormat {
    type Err = ferris_core::Error;

    /// Parse the wire spelling (`pdf`/`svg`/`png`/`html`).
    ///
    /// # Errors
    ///
    /// Returns a validation error naming the accepted spellings.
    fn from_str(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "pdf" => Ok(Self::Pdf),
            "svg" => Ok(Self::Svg),
            "png" => Ok(Self::Png),
            "html" => Ok(Self::Html),
            other => Err(Error::Validation(ValidationError::Invalid(format!(
                "unsupported print output format `{other}`; expected pdf, svg, png or html"
            )))),
        }
    }
}

/// Template language a format is written in.
///
/// Both engines end in Typst: `Typst` compiles the source as-is, while `Jinja`
/// runs the source through `MiniJinja` first and compiles the Typst it produces.
/// That keeps one typesetter (fonts, paper, PDF) for every format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormatEngine {
    #[default]
    Typst,
    Jinja,
}

impl std::str::FromStr for FormatEngine {
    type Err = ferris_core::Error;

    /// Parse the stored spelling. `Server` is the historical value for a raw
    /// Typst layout and is accepted so existing rows keep working.
    ///
    /// # Errors
    ///
    /// Returns a validation error naming the supported engines.
    fn from_str(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "typst" | "server" => Ok(Self::Typst),
            "jinja" | "jinja2" | "minijinja" => Ok(Self::Jinja),
            other => Err(Error::Validation(ValidationError::Invalid(format!(
                "unsupported print engine `{other}`; expected jinja or typst"
            )))),
        }
    }
}

/// Paper size.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaperSize {
    A4,
    A3,
    A5,
    Letter,
    Legal,
    Tabloid,
    Custom(f32, f32), // width, height in mm
}

impl PaperSize {
    #[must_use]
    pub const fn dimensions(&self) -> (f32, f32) {
        match self {
            Self::A4 => (210.0, 297.0),
            Self::A3 => (297.0, 420.0),
            Self::A5 => (148.0, 210.0),
            Self::Letter => (215.9, 279.4),
            Self::Legal => (215.9, 355.6),
            Self::Tabloid => (279.4, 431.8),
            Self::Custom(w, h) => (*w, *h),
        }
    }

    /// The spelling stored in `Print Format`/`Print Settings`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::A4 => "A4",
            Self::A3 => "A3",
            Self::A5 => "A5",
            Self::Letter => "Letter",
            Self::Legal => "Legal",
            Self::Tabloid => "Tabloid",
            Self::Custom(_, _) => "Custom",
        }
    }
}

impl std::str::FromStr for PaperSize {
    type Err = ferris_core::Error;

    /// Parse the stored/wire spelling of a paper size.
    ///
    /// # Errors
    ///
    /// Returns a validation error naming the accepted sizes.
    fn from_str(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "a4" => Ok(Self::A4),
            "a3" => Ok(Self::A3),
            "a5" => Ok(Self::A5),
            "letter" | "us-letter" => Ok(Self::Letter),
            "legal" | "us-legal" => Ok(Self::Legal),
            "tabloid" | "us-tabloid" => Ok(Self::Tabloid),
            other => Err(Error::Validation(ValidationError::Invalid(format!(
                "unknown paper size `{other}`; expected a3, a4, a5, letter, legal or tabloid"
            )))),
        }
    }
}

/// Page margins in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Margins {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
}

impl Default for Margins {
    fn default() -> Self {
        Self {
            top: 20.0,
            bottom: 20.0,
            left: 20.0,
            right: 20.0,
        }
    }
}

/// Page orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    Portrait,
    Landscape,
}

impl Orientation {
    /// The spelling stored in `Print Format`/`Print Settings`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Portrait => "Portrait",
            Self::Landscape => "Landscape",
        }
    }
}

impl std::str::FromStr for Orientation {
    type Err = ferris_core::Error;

    /// Parse the stored/wire spelling of the orientation.
    ///
    /// # Errors
    ///
    /// Returns a validation error naming the accepted values.
    fn from_str(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "portrait" => Ok(Self::Portrait),
            "landscape" => Ok(Self::Landscape),
            other => Err(Error::Validation(ValidationError::Invalid(format!(
                "unknown orientation `{other}`; expected portrait or landscape"
            )))),
        }
    }
}

/// Install-wide print defaults, read from the `Print Settings` Single.
///
/// A format's own values win; these are what a format (or the builtin fallback)
/// starts from when the row leaves a value empty.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PrintSettings {
    pub paper_size: PaperSize,
    pub orientation: Orientation,
    pub scale: f32,
}

impl Default for PrintSettings {
    fn default() -> Self {
        Self {
            paper_size: PaperSize::A4,
            orientation: Orientation::Portrait,
            scale: 1.0,
        }
    }
}

impl PrintSettings {
    /// Read the Single's row. Unknown or malformed values fall back to the
    /// default for that field, so a half-edited settings row cannot break
    /// printing for every `DocType`.
    #[must_use]
    pub fn from_value(value: &Value) -> Self {
        let mut settings = Self::default();
        if let Some(raw) = value.get("paper_size").and_then(Value::as_str)
            && let Ok(paper) = raw.parse::<PaperSize>()
        {
            settings.paper_size = paper;
        }
        if let Some(raw) = value.get("orientation").and_then(Value::as_str)
            && let Ok(orientation) = raw.parse::<Orientation>()
        {
            settings.orientation = orientation;
        }
        if let Some(scale) = value.get("scale").and_then(Value::as_f64)
            && scale.is_finite()
            && scale > 0.0
            && scale <= f64::from(f32::MAX)
        {
            // `TryFrom<f64> for f32` is still unstable, so the range check
            // above is what makes the narrowing cast lossless for this field.
            #[allow(clippy::cast_possible_truncation)]
            {
                settings.scale = scale as f32;
            }
        }
        settings
    }
}

/// Print format definition: everything the renderer needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintFormatDef {
    pub name: String,
    pub doctype: String,
    pub description: String,
    pub engine: FormatEngine,
    /// Source in `engine`'s language. For `Jinja`, what `MiniJinja` produces is
    /// Typst source.
    pub template: String,
    /// Relative folder under the host's print template root. It scopes both
    /// Jinja `{% include %}`/`{% extends %}` and Typst `read()`.
    pub template_dir: Option<String>,
    /// Default values for the template's `params` object. A request may
    /// override individual keys.
    pub parameters: HashMap<String, Value>,
    /// Data a preview uses when the caller does not send a sample document.
    pub sample_data: Value,
    pub paper_size: PaperSize,
    pub margins: Margins,
    pub orientation: Orientation,
    pub scale: f32,
    pub enabled: bool,
}

impl PrintFormatDef {
    #[must_use]
    pub fn new(name: String, doctype: String, template: String) -> Self {
        Self {
            name,
            doctype,
            description: String::new(),
            engine: FormatEngine::Typst,
            template,
            template_dir: None,
            parameters: HashMap::new(),
            sample_data: Value::object(HashMap::new()),
            paper_size: PaperSize::A4,
            margins: Margins::default(),
            orientation: Orientation::Portrait,
            scale: 1.0,
            enabled: true,
        }
    }

    #[must_use]
    pub fn new_jinja(name: String, doctype: String, template: String) -> Self {
        Self {
            engine: FormatEngine::Jinja,
            ..Self::new(name, doctype, template)
        }
    }

    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    #[must_use]
    pub fn with_parameters(mut self, parameters: HashMap<String, Value>) -> Self {
        self.parameters = parameters;
        self
    }

    #[must_use]
    pub fn with_template_dir(mut self, template_dir: impl Into<String>) -> Self {
        self.template_dir = Some(template_dir.into());
        self
    }

    #[must_use]
    pub fn with_sample_data(mut self, sample_data: Value) -> Self {
        self.sample_data = sample_data;
        self
    }
}

/// Options for a single render. Values that are `None` fall back to the
/// format's own defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintOptions {
    pub format: PrintFormat,
    pub paper_size: Option<PaperSize>,
    pub margins: Option<Margins>,
    pub orientation: Option<Orientation>,
    pub scale: Option<f32>,
    pub watermark: Option<String>,
    /// Request-level overrides for the format's `parameters`.
    #[serde(default)]
    pub params: HashMap<String, Value>,
    /// Document metadata exposed to the template as `meta`.
    #[serde(default)]
    pub meta: Option<Value>,
    /// The asking user exposed to the template as `user`.
    #[serde(default)]
    pub user: Option<Value>,
    /// Clock override, so a preview or a test can pin `now`.
    #[serde(default)]
    pub now: Option<DateTime<Utc>>,
}

impl Default for PrintOptions {
    fn default() -> Self {
        Self {
            format: PrintFormat::Pdf,
            paper_size: None,
            margins: None,
            orientation: None,
            scale: None,
            watermark: None,
            params: HashMap::new(),
            meta: None,
            user: None,
            now: None,
        }
    }
}
