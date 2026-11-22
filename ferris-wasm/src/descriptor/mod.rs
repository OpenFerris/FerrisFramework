//! Host-facing projection of the app's typed `descriptor`.
//!
//! The WIT `interface descriptor` defines the shape of `describe()`; the
//! `bindgen!` expansion hands it to the host as typed records. This module
//! adapts those generated types to the host surfaces that consume them:
//!
//! - doctypes → `ferris_schema::DoctypeMeta` (`doctype_meta`/`plugin_field_meta`),
//! - themes / folders / workspaces → the `ferris-core` registries,
//! - methods → [`MethodSpec`],
//! - the whole descriptor → [`DescriptorJson`], the stable host-side JSON the
//!   dev endpoint `/plugins/{name}` serves (projected from typed data, not by
//!   echoing the guest's payload back).
//!
//! Nothing here **decodes** app JSON anymore: the plugin
//! boundary is the generated records, so the free-form JSON shapes that drifted
//! out of sync in 1.x are gone. The `Config*` mirrors below exist only to
//! *serialize* the projection for the endpoint and share their shape with the
//! contract by construction.
//!
//! Split by concern: `doctype` projects a declaration into schema metadata,
//! `registries` adapts the theme/workspace/widget specs, `json` is the wire
//! mirror the dev endpoint serializes, and `method` is the API method spec.

mod doctype;
mod json;
mod method;
mod network;
mod registries;

pub use doctype::{ConfigDoctype, ConfigField, ConfigIndex, FieldInput, doctype_meta};
pub use json::{
    ConfigJson, DescriptorJson, HttpPolicyJson, RouteJson, SubscriptionJson, TlsClientJson,
    TlsServerJson,
};
pub use method::MethodSpec;
pub use network::{NetworkSpec, RouteKind, RouteSpec};
pub use registries::{
    dashboard_spec, ui_extension_spec, website_spec, widget_def_spec, workspace_spec,
};

#[cfg(test)]
mod tests;
