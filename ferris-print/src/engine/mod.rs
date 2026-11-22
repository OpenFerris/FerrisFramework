//! PDF generation engine using Typst.
//!
//! The engine is split by concern: config holds the wire metadata, template
//! renders `MiniJinja` or returns raw Typst, options merges a request's options
//! with a format's defaults, world loads the Typst resources and builds the
//! per-render world, export compiles and serialises, and `print_engine` is the
//! public façade.

mod config;
mod export;
mod options;
mod print_engine;
mod template;
mod world;

pub use config::{
    FormatEngine, Margins, Orientation, PaperSize, PrintConfig, PrintFormat, PrintFormatDef,
    PrintOptions, PrintSettings, TemplateLimits,
};
pub use print_engine::PrintEngine;

#[cfg(test)]
mod tests;
