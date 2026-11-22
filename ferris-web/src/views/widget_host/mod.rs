//! Phase 1: one widget instance → its body in the dashboard grid.
//!
//! `dashboard_view` no longer decides what to draw: it hands each instance to
//! [`WidgetHost`], which resolves it against the feed's catalog
//! ([`UiCtx::resolve_widget_ref`](crate::ext::UiCtx::resolve_widget_ref)) and
//! renders the body its definition's owner implies:
//!
//! - **`host.*`** — the Desk's own renderers (`host.stat`, `host.trend`,
//!   `host.table`): the data comes from `DocStore` as the current session, not
//!   through `/plugins/*`.
//! - **any other owner** — the plugin's custom element: an SSR marker for
//!   `/plugins/{owner}/widget/{reference}?params=…` while the script loads, and
//!   the live element with `data-params` afterwards.
//! - **unresolved** — a dashed marker naming the reference; the rest of the
//!   dashboard keeps rendering.
//!
//! Each instance resolves to a *body*: [`WidgetHost`] no longer decides where it
//! lives or how much of the grid it occupies. That is the dashboard's job
//! (`views::dashboard`), which hands it the placed tile
//! (`views::dashboard::DashboardTile`) and an optional `refresh` to reload the
//! native widgets.
//!
//! Split by concern: [`host`] owns the component and its heading, [`native`]
//! the `host.*` renderers, [`params`] the instance-parameter lookup, [`code`]
//! the plugin custom-element mount, and [`placeholder`] the bodies for an
//! unresolved or misconfigured instance.

mod code;
mod host;
mod native;
mod params;
mod placeholder;

#[cfg(test)]
mod tests;

pub use host::WidgetHost;
