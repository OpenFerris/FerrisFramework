//! A small MDX/Markdown renderer.
//
//! The mapping targets MDX compiled with a JS toolchain, which is not available
//! here. This module implements the static subset that matters for CMS pages:
//! YAML-ish front matter plus common Markdown (headings, paragraphs, emphasis,
//! inline/​fenced code, links, lists and blockquotes). Raw HTML is escaped.

pub mod renderer;

pub use ferris_web_shared::markdown::{FrontMatter, front_matter, parse_front_matter};
pub use renderer::{RenderedMarkdown, render, render_page};
