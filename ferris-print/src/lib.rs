//! Ferris Print — user-authored print formats, rendered with Typst.
//!
//! The crate has two layers:
//!
//! * a pure renderer ([`PrintEngine`]): a [`PrintFormatDef`] plus a document
//!   becomes PDF/SVG/PNG/HTML bytes, synchronously and without database or
//!   network access;
//! * an optional host adapter behind the `http` feature ([`PrintModule`]):
//!   `Print Format` rows, `Print Settings`, permissions, preview and the REST
//!   routes.
//!
//! Formats are centralized in the `Print Format` `DocType`. A row references the
//! Document Type it prints, chooses a `jinja` or `typst` engine, carries the
//! template, default parameters and sample data, and owns its page geometry.
//! There is no second registry: the database row is the format.

// `ferris_core::Error` is the workspace-wide error type and is intentionally
// unboxed: boxing it only at the print boundary would make every caller match
// on a different shape from the rest of the framework.
#![allow(clippy::result_large_err)]

pub mod doctypes;
pub mod engine;
#[cfg(feature = "http")]
pub mod http;
pub mod templates;
pub mod workspaces;

pub use engine::{
    FormatEngine, Margins, Orientation, PaperSize, PrintConfig, PrintEngine, PrintFormat,
    PrintFormatDef, PrintOptions, PrintSettings, TemplateLimits,
};
#[cfg(feature = "http")]
pub use http::PrintModule;
pub use templates::{BUILTIN_TEMPLATES, GENERIC_TEMPLATE};
