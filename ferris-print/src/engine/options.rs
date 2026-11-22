//! Resolution of the request's print options against a format's defaults.

use std::fmt::Write as _;

use ferris_core::{Error, Result, ValidationError};

use super::config::{Margins, Orientation, PaperSize, PrintFormat, PrintFormatDef, PrintOptions};

/// Lowest `scale` the renderer accepts. Below it a page is unreadable and a PNG
/// is a thumbnail, so the value is almost always a mistake.
pub(super) const MIN_SCALE: f32 = 0.1;

/// Highest `scale` the renderer accepts.
///
/// PNG is the reason the ceiling is hard: `engine::export` renders at
/// `scale * 2.0` pixels per point (144 dpi at 1.0) and allocates
/// `width * height * 4` bytes, so an unbounded scale is an out-of-memory abort
/// rather than a large file (PRINT-02). A4 at 4.0 is ~32 M pixels (~128 MiB).
pub(super) const MAX_SCALE: f32 = 4.0;

/// RGBA pixels one PNG render may allocate for its first page.
///
/// A4 at `4.0` fits (~32 M); Tabloid at `4.0` (~62 M) does not, and must be
/// reduced. The ceiling is deliberately below what a 32-bit process can
/// allocate so the error is a validation error, not an abort.
pub(super) const MAX_PIXELS: u64 = 40_000_000;

// The comparison happens on a computed pixel count, never on user input, and
// 40 M is far inside f64's exact-integer range, so the lossy-looking cast is
// exact here.
#[allow(clippy::cast_precision_loss)]
const MAX_PIXELS_F64: f64 = MAX_PIXELS as f64;

/// Pixels per millimetre at scale 1.0 (144 dpi), matching `engine::export`.
const PIXELS_PER_MM: f64 = 144.0 / 25.4;

/// The options that actually reach the Typst source, after merging the
/// format's defaults and validating what the renderer can honour.
#[derive(Debug)]
pub(super) struct ResolvedPrint {
    pub(super) format: PrintFormat,
    pub(super) paper: PaperSize,
    pub(super) margins: Margins,
    pub(super) orientation: Orientation,
    /// Output scale: a pixel ratio for PNG, a content scale for paged exports.
    pub(super) scale: f32,
    pub(super) watermark: Option<String>,
}

impl ResolvedPrint {
    pub(super) fn resolve(format: &PrintFormatDef, options: &PrintOptions) -> Result<Self> {
        let paper = options.paper_size.unwrap_or(format.paper_size);
        let (width, height) = paper.dimensions();
        if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
            return Err(Error::Validation(ValidationError::Invalid(
                "paper dimensions must be positive".to_string(),
            )));
        }
        let margins = options.margins.unwrap_or(format.margins);
        if [margins.top, margins.bottom, margins.left, margins.right]
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(Error::Validation(ValidationError::Invalid(
                "margins must be finite and non-negative".to_string(),
            )));
        }
        let orientation = options.orientation.unwrap_or(format.orientation);
        let scale = options.scale.unwrap_or(format.scale);
        if !(scale.is_finite() && (MIN_SCALE..=MAX_SCALE).contains(&scale)) {
            return Err(Error::Validation(ValidationError::Invalid(format!(
                "scale must be between {MIN_SCALE} and {MAX_SCALE}, got {scale}"
            ))));
        }
        if matches!(options.format, PrintFormat::Html) && (scale - 1.0).abs() > f32::EPSILON {
            return Err(unsupported("`scale` for HTML output"));
        }
        // A PNG is rasterised at `scale * 2.0` pixels per point; a page whose
        // raster would exceed the ceiling is refused here, before Typst
        // allocates it. Paged formats scale the content instead, so the cap is
        // meaningful for PNG only.
        if matches!(options.format, PrintFormat::Png) {
            let pixels = f64::from(width)
                * PIXELS_PER_MM
                * f64::from(scale)
                * (f64::from(height) * PIXELS_PER_MM * f64::from(scale));
            if pixels > MAX_PIXELS_F64 {
                return Err(Error::Validation(ValidationError::Invalid(format!(
                    "PNG at scale {scale} on {}x{} mm would be {} pixels; the limit is \
                     {MAX_PIXELS}. Reduce `scale` or the paper size",
                    width,
                    height,
                    pixels.round()
                ))));
            }
        }
        let watermark = options
            .watermark
            .clone()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty());

        Ok(Self {
            format: options.format,
            paper,
            margins,
            orientation,
            scale,
            watermark,
        })
    }

    /// The Typst statements prepended to the template.
    ///
    /// `#set page` can be redefined by the template (Typst applies the last
    /// setting in scope); that is documented rather than fought, but the
    /// request's values are what the page starts with.
    pub(super) fn page_prelude(&self) -> String {
        let (width, height) = self.paper.dimensions();
        let paper = match self.paper {
            PaperSize::A4 => "\"a4\"".to_string(),
            PaperSize::A3 => "\"a3\"".to_string(),
            PaperSize::A5 => "\"a5\"".to_string(),
            PaperSize::Letter => "\"us-letter\"".to_string(),
            PaperSize::Legal => "\"us-legal\"".to_string(),
            PaperSize::Tabloid => "\"us-tabloid\"".to_string(),
            PaperSize::Custom(_, _) => format!("(width: {width}mm, height: {height}mm)"),
        };
        let flipped = matches!(self.orientation, Orientation::Landscape);
        let mut prelude = format!(
            "#set page(paper: {paper}, margin: (top: {}mm, bottom: {}mm, left: {}mm, right: {}mm), flipped: {flipped}",
            self.margins.top, self.margins.bottom, self.margins.left, self.margins.right
        );
        if let Some(watermark) = &self.watermark {
            let _ = write!(
                prelude,
                ", background: place(center + horizon, rotate(-45deg, text(size: 64pt, fill: luma(200), \"{}\")))",
                escape_typst_string(watermark)
            );
        }
        prelude.push_str(")\n");
        // PNG's `scale` is the pixel ratio applied at export; the paged formats
        // have no such knob, so the content itself is scaled.
        if !matches!(self.format, PrintFormat::Png) && (self.scale - 1.0).abs() > f32::EPSILON {
            let percent = self.scale * 100.0;
            let _ = writeln!(
                prelude,
                "#show: doc => scale(x: {percent}%, y: {percent}%, reflow: true, doc)"
            );
        }
        prelude
    }
}

/// A precise "this option is not implemented" error.
fn unsupported(what: &str) -> Error {
    Error::Validation(ValidationError::Invalid(format!(
        "unsupported print option: {what}"
    )))
}

/// Escape a value for a Typst string literal (`"..."`).
fn escape_typst_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format() -> PrintFormatDef {
        PrintFormatDef::new(
            "fixture".to_string(),
            "Note".to_string(),
            "= body".to_string(),
        )
    }

    fn options(format: PrintFormat, scale: f32) -> PrintOptions {
        PrintOptions {
            format,
            scale: Some(scale),
            ..PrintOptions::default()
        }
    }

    #[test]
    fn a_scale_outside_the_supported_range_is_refused() {
        for scale in [0.0, -0.5, 0.05, 4.5, f32::INFINITY, f32::NAN] {
            let error = ResolvedPrint::resolve(&format(), &options(PrintFormat::Pdf, scale))
                .expect_err("an out-of-range scale is a validation error");
            assert!(error.to_string().contains("scale"), "{scale}: {error}");
        }
        for scale in [MIN_SCALE, 1.0, MAX_SCALE] {
            assert!(
                ResolvedPrint::resolve(&format(), &options(PrintFormat::Pdf, scale)).is_ok(),
                "{scale} is inside the range"
            );
        }
    }

    #[test]
    fn a_png_over_the_pixel_ceiling_is_refused() {
        let mut tabloid = format();
        tabloid.paper_size = PaperSize::Tabloid;

        let error = ResolvedPrint::resolve(&tabloid, &options(PrintFormat::Png, 4.0))
            .expect_err("Tabloid at 4x exceeds the pixel ceiling");
        assert!(error.to_string().contains("pixels"), "{error}");

        // The same page at 1.0 fits, and a paged format scales content instead
        // of allocating a raster, so it has no pixel ceiling.
        assert!(ResolvedPrint::resolve(&tabloid, &options(PrintFormat::Png, 1.0)).is_ok());
        assert!(ResolvedPrint::resolve(&tabloid, &options(PrintFormat::Pdf, 4.0)).is_ok());
    }
}
