//! Wire shapes of the print endpoints.

use std::collections::HashMap;

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use ferris_core::{Error, Result, Value};
use serde::Deserialize;

use crate::{Margins, Orientation, PaperSize, PrintFormat, PrintOptions};

/// JSON body of `POST /api/print/{doctype}/{name}`.
#[derive(Debug, Default, Deserialize)]
pub struct PrintRequest {
    /// Output format: `pdf` (default), `svg`, `png` or `html`.
    #[serde(default)]
    pub format: Option<String>,
    /// Which `Print Format` row to use. Absent = the first enabled one, or the
    /// builtin generic layout when the `DocType` has none.
    #[serde(default)]
    pub print_format: Option<String>,
    #[serde(default)]
    pub paper_size: Option<String>,
    #[serde(default)]
    pub margins: Option<Margins>,
    #[serde(default)]
    pub orientation: Option<String>,
    #[serde(default)]
    pub scale: Option<f32>,
    #[serde(default)]
    pub watermark: Option<String>,
    /// Per-render overrides for the format's `parameters`.
    #[serde(default)]
    pub params: HashMap<String, Value>,
}

impl PrintRequest {
    /// The `PrintOptions` the engine resolves against the format's defaults.
    ///
    /// # Errors
    ///
    /// A validation error for an unknown output format, paper size or
    /// orientation.
    pub fn options(&self) -> Result<PrintOptions> {
        Ok(PrintOptions {
            format: self
                .format
                .as_deref()
                .unwrap_or("pdf")
                .parse::<PrintFormat>()?,
            paper_size: self
                .paper_size
                .as_deref()
                .map(str::parse::<PaperSize>)
                .transpose()?,
            margins: self.margins,
            orientation: self
                .orientation
                .as_deref()
                .map(str::parse::<Orientation>)
                .transpose()?,
            scale: self.scale,
            watermark: self.watermark.clone(),
            params: self.params.clone(),
            meta: None,
            user: None,
            now: None,
        })
    }
}

/// JSON body of `POST /api/print/preview/{doctype}`.
///
/// A preview either names a stored `Print Format` or sends an inline
/// `template`/`engine` pair (what a visual editor sends before saving). It
/// never reads a document, so it is gated by the `DocType`'s `print` right.
#[derive(Debug, Default, Deserialize)]
pub struct PreviewRequest {
    #[serde(flatten)]
    pub print: PrintRequest,
    /// Inline source. Mutually exclusive with `print.print_format`.
    #[serde(default)]
    pub template: Option<String>,
    /// Inline engine: `jinja` (default) or `typst`.
    #[serde(default)]
    pub engine: Option<String>,
    /// Relative folder under the host template root for includes/extends.
    #[serde(default)]
    pub template_dir: Option<String>,
    /// Sample document. Absent = the format's own `sample_data`.
    #[serde(default)]
    pub data: Option<Value>,
}

/// A rendered document ready to become an HTTP response.
#[derive(Debug)]
pub struct PrintOutput {
    pub format: PrintFormat,
    pub bytes: Vec<u8>,
}

impl PrintOutput {
    #[must_use]
    pub const fn content_type(&self) -> &'static str {
        match self.format {
            PrintFormat::Pdf => "application/pdf",
            PrintFormat::Svg => "image/svg+xml",
            PrintFormat::Png => "image/png",
            PrintFormat::Html => "text/html; charset=utf-8",
        }
    }

    #[must_use]
    pub const fn extension(&self) -> &'static str {
        match self.format {
            PrintFormat::Pdf => "pdf",
            PrintFormat::Svg => "svg",
            PrintFormat::Png => "png",
            PrintFormat::Html => "html",
        }
    }

    /// Build the binary response with its content type and a safe filename.
    pub fn into_response(self, doctype: &str, name: &str) -> Response {
        let filename = format!(
            "{}-{}.{}",
            sanitize_filename(doctype),
            sanitize_filename(name),
            self.extension()
        );
        let mut response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, self.content_type())
            .body(Body::from(self.bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        // A file name is caller-influenced (the document name); sanitise it and
        // skip the header rather than fail the render if it still cannot be
        // encoded.
        if let Ok(value) = HeaderValue::from_str(&format!("inline; filename=\"{filename}\"")) {
            response
                .headers_mut()
                .insert(header::CONTENT_DISPOSITION, value);
        }
        response
    }
}

/// Keep a document/doctype name usable as a file name.
pub fn sanitize_filename(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "document".to_string()
    } else {
        out
    }
}

/// Convert a serializable value into the crate's [`Value`], as a typed error
/// instead of an `unwrap`.
pub fn to_value<T: serde::Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value)
        .map(Value::from_json)
        .map_err(|error| Error::Serialization(ferris_core::SerializationError::Json(error)))
}
