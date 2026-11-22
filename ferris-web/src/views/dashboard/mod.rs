//! Dashboard grid: read view and *builder* (drag/resize, palette,
//! per-instance settings).
//!
//! A dashboard is a 12-column grid of [`WidgetInstance`]s. This module is the
//! Desk half of a contract the server also enforces: placement
//! (column/row/span/rows) is resolved with
//! [`ferris_schema::contract::layout`], the same rule that
//! `ferris-api::dashboard_layout` uses when saving, so what the user sees
//! is what the next load reproduces.
//!
//! It was split into files by responsibility (the old `dashboard_editor.rs`
//! was a 799-line monolith):
//!
//! - [`model`] — geometry: catalog sizing, normalization, the tile/board
//!   inline styles and the catalog grouped by category for the palette.
//! - [`params`] — the `WidgetParam` → `FormField` translation the properties
//!   panel renders and validates through (the same field system the
//!   document form uses).
//! - [`card`] — [`card::DashboardTile`]: the one tile both faces share — the
//!   placed box, the body, and (in edit mode) the chrome and the pointer
//!   gestures.
//! - [`canvas`] — [`canvas::DashboardBoard`]: the keyed `<For>` board and the
//!   gesture state.
//! - [`config`] — [`config::BoardConfig`]: the board's configuration drawer
//!   (geometry, surface, chrome flags) over a live draft.
//! - [`drawer`] — [`drawer::WidgetPalette`]: the catalog for adding widgets.
//! - [`preview`] — the theme-aware preview image (sanitized inline SVG).
//! - [`props`] — [`props::WidgetProps`]: title, grouped parameters and size.
//! - [`style`] — the appearance precedence (global → declared → override) and
//!   the CSS custom properties each level emits.
//! - [`style_fields`] — the appearance controls both panels share (surface,
//!   token, number, shadow).
//! - [`view`] — [`view::DashboardGrid`]: loading, top-bar publication and
//!   wiring.

mod canvas;
mod card;
mod config;
mod drawer;
mod model;
mod params;
mod preview;
mod props;
mod style;
mod style_fields;
mod view;

pub use view::DashboardGrid;
