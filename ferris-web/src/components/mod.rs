//! Desk components.
//!
//! The reusable Leptos library lives in `ferris-web-ui`; it is re-exported here
//! under its historical paths so `ferris_web::components::ui::…` keeps
//! resolving for callers inside this crate and for the integration tests.

pub use ferris_web_ui::components::{constants, desk, dropdown_builder, hooks, ui, utils};

pub mod pickers;
pub mod views;
