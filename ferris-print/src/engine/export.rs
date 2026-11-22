//! Compilation of a world and export to each output format.

use chrono::{DateTime, Datelike, Timelike, Utc};
use ferris_core::{Error, Result, ValidationError};
use tracing::warn;
use typst::compile;
use typst::diag::{SourceResult, Warned};
use typst::foundations::Datetime;
use typst::html::HtmlDocument;
use typst::layout::{Abs, PagedDocument};
use typst_pdf::{PdfOptions, Timestamp};
use typst_render::render;
use typst_svg::svg_merged;

use super::config::PrintFormat;
use super::world::TypstWorld;

/// Export a document to its HTML representation.
#[allow(clippy::needless_lifetimes)]
fn html_export(document: &HtmlDocument) -> SourceResult<String> {
    typst_html::html(document)
}

/// Convert a Typst [`Datetime`] into a PDF creation timestamp at UTC.
// The `as u8` casts are safe: chrono's month/day/hour/minute/second are in
// 1..=12 / 1..=31 / 0..=23 / 0..=59 / 0..=60, which fit a `u8`.
#[allow(clippy::cast_possible_truncation)]
fn convert_datetime(dt: DateTime<Utc>) -> Option<Datetime> {
    Datetime::from_ymd_hms(
        dt.year(),
        dt.month() as u8,
        dt.day() as u8,
        dt.hour() as u8,
        dt.minute() as u8,
        dt.second() as u8,
    )
}

/// Map Typst diagnostics into a ferris error.
fn diagnostics_error(what: &str, diagnostics: &[typst::diag::SourceDiagnostic]) -> Error {
    if diagnostics.is_empty() {
        return Error::Validation(ValidationError::Invalid(format!(
            "{what}: typst compilation failed"
        )));
    }
    let detail = diagnostics
        .iter()
        .map(|d| d.message.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    Error::Validation(ValidationError::Invalid(format!("{what}: {detail}")))
}

/// Compile a paged document, logging warnings.
fn compile_paged(world: &TypstWorld) -> Result<PagedDocument> {
    let Warned { output, warnings } = compile::<PagedDocument>(world);
    log_warnings(&warnings);
    output.map_err(|diags| diagnostics_error("typst compilation failed", &diags))
}

/// Compile an HTML document, logging warnings.
fn compile_html_document(world: &TypstWorld) -> Result<HtmlDocument> {
    let Warned { output, warnings } = compile::<HtmlDocument>(world);
    log_warnings(&warnings);
    output.map_err(|diags| diagnostics_error("typst compilation failed", &diags))
}

fn log_warnings(warnings: &[typst::diag::SourceDiagnostic]) {
    for warning in warnings {
        warn!("typst warning: {}", warning.message);
    }
}

/// Build the output bytes for a given export format.
pub(super) fn export(world: &TypstWorld, format: PrintFormat, scale: f32) -> Result<Vec<u8>> {
    match format {
        PrintFormat::Pdf => {
            let document = compile_paged(world)?;
            let timestamp = convert_datetime(Utc::now()).map(Timestamp::new_utc);
            let options = PdfOptions {
                timestamp,
                ..PdfOptions::default()
            };
            typst_pdf::pdf(&document, &options)
                .map_err(|diags| diagnostics_error("PDF export failed", &diags))
        }
        PrintFormat::Svg => {
            let document = compile_paged(world)?;
            Ok(svg_merged(&document, Abs::zero()).into_bytes())
        }
        PrintFormat::Png => {
            let document = compile_paged(world)?;
            let page = document.pages.first().ok_or_else(|| {
                ferris_core::FileError::Storage(
                    "PNG export failed: document has no pages".to_string(),
                )
            })?;
            // 144 dpi at scale 1.0.
            let pixmap = render(page, scale * 2.0);
            pixmap.encode_png().map_err(|e| {
                ferris_core::Error::from(ferris_core::FileError::Storage(format!(
                    "PNG export failed: {e}"
                )))
            })
        }
        PrintFormat::Html => {
            let document = compile_html_document(world)?;
            html_export(&document)
                .map(String::into_bytes)
                .map_err(|diags| diagnostics_error("HTML export failed", &diags))
        }
    }
}
