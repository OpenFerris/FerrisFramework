//! OpenUI slot runtime (Fase 1): the Desk mounts plugin components into
//! host-defined slots.
//!
//! Plugins declare `"ui_extensions"` in their `describe()`: a list of pairs of a
//! **slot** (a host-defined, documented mount point such as `form.toolbar.right`
//! or `list.toolbar.bulk`) and a **component** (a custom element the plugin
//! defines, e.g. `"invoice-actions"` → `<ferris-invoice-actions>`). The host
//! registers those declarations server-side (`ferris_core::ui_extensions`);
//! this module fetches the feed (`GET /api/ui-extensions`) on the client and
//! mounts the matching elements. The manifest stays live (Fase 4): the feed
//! is refetched when the tab returns to the foreground and after login, and
//! the payload's `revision` gates the swap so an unchanged feed costs zero
//! re-renders.
//!
//! A slot with no declared extension renders nothing: empty anchor, zero bytes.
//! The closer that mounts nothing does not need to know — it renders `<UiSlot>`
//! exactly where the extension belongs and the runtime fills it in.
//!
//! Communication is the Web Components contract: the host sets `data-*`
//! attributes (slot, doctype, label) as the initial props, creates the element
//! inside the anchor, and the plugin's element talks back with Custom Events.
//!
//! [`SLOT_FORM_VIEW`] is special: it is the one **whole-view** slot, mounted
//! *instead of* the metadata form (see [`doctype_view`]) rather than in
//! addition to it, so an app can own a doctype's editing experience end to end.
//!
//! Split by concern: [`feed`] owns the feed state and the resolution rules,
//! [`slots`] the slot constants and components, [`mount`] the wasm DOM mount,
//! [`assets`] the custom-element naming, and the sibling [`loader`] module the
//! plugin asset injection.

pub use ferris_schema::contract::{
    AppUi, Dashboard, UiExtension, UiFeed, Website, WebsiteRoute, WidgetDef, WidgetInstance,
};

mod assets;
mod feed;
mod mount;
mod slots;

#[cfg(test)]
mod tests;

/// Plugin asset loading for the Desk: injects the scripts/stylesheets that
/// define a plugin's custom elements before they are mounted into slots.
pub mod loader;

/// App-declared custom doctype views: the [`SLOT_FORM_VIEW`] mount point.
pub mod doctype_view;

pub use doctype_view::DoctypeViewHost;

/// Lazy loader for the vendored third-party browser libraries:
/// Leaflet, JsBarcode, Quill and marked, served from `/static/vendor/*` and
/// injected the first time a form widget needs them.
pub mod vendor;

#[cfg(target_arch = "wasm32")]
pub(crate) use assets::component_tag;
pub use feed::{UiCtx, form_view_in, resolve_ref_in};
pub use slots::{
    SLOT_FORM_AFTER_FIELDS, SLOT_FORM_BEFORE_FIELDS, SLOT_FORM_SIDEBAR_BOTTOM,
    SLOT_FORM_SIDEBAR_TOP, SLOT_FORM_TOOLBAR_LEFT, SLOT_FORM_TOOLBAR_RIGHT, SLOT_FORM_VIEW,
    SLOT_LIST_BEFORE_TABLE, SLOT_LIST_ROW_ACTIONS, SLOT_LIST_TOOLBAR_BULK, SLOT_SHELL_HEADER,
    SLOT_SHELL_SIDEBAR_BOTTOM, SLOT_SHELL_TOPBAR_RIGHT, UiHeaderSlot, UiSlot,
};

#[cfg(target_arch = "wasm32")]
pub(crate) use mount::resolve_props;
