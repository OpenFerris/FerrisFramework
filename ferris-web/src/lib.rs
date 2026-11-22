//! Ferris Web - Admin UI (Leptos SSR + WASM)

#![recursion_limit = "512"]
#![forbid(unsafe_code)]

// `leptos_axum` is used by the SSR binary and the route-list test; the lib
// itself only names it so the dependency is accounted for in an SSR build.
#[cfg(feature = "ssr")]
use leptos_axum as _;

pub mod api;
pub mod app;
pub mod components;
pub mod desk;
#[cfg(feature = "ssr")]
pub mod doctypes;
pub mod ext;
#[cfg(all(feature = "hydrate", target_arch = "wasm32"))]
pub mod hydrate;
pub mod markdown;
pub mod shell;
pub mod svg;
#[cfg(test)]
pub(crate) mod test_support;
pub mod theme;
pub mod views;
// Server-side only: the browser fetches these bytes over HTTP, so they must
// never be linked into the wasm bundle (same reasoning as `DESK_CSS`).
// `preview_assets` = widget-catalog previews; `vendor_assets` = vendored
// third-party libraries.
#[cfg(not(target_arch = "wasm32"))]
pub mod preview_assets;
#[cfg(not(target_arch = "wasm32"))]
pub mod vendor_assets;
pub mod web_user;
#[cfg(feature = "ssr")]
pub mod website;
#[cfg(feature = "ssr")]
pub mod workspaces;

/// Compiled desk stylesheet. Generated from `style/tailwind.css` (source) with:
/// ``cd ferris-web && node_modules/.bin/tailwindcss -i style/tailwind.css -o style/desk.css --minify``
/// Served by the framework at `/static/desk.css`.
///
/// Gated on `ssr` because the bytes are only ever read by the server: the
/// browser fetches the stylesheet over HTTP instead of carrying a second copy
/// inside the wasm module.
#[cfg(feature = "ssr")]
pub const DESK_CSS: &str = include_str!("../style/desk.css");

pub use app::{BareChrome, DeskApp, DeskNav, LoginComplete, provide_bare_chrome, use_bare_chrome};
pub use shell::DeskShell;
pub use web_user::WebUser;
#[cfg(feature = "ssr")]
pub use website::{
    Blog, BlogPost, Comment, Discussion, RenderedPage, WebForm, WebFormField, WebPage,
    WebsiteRouter,
};
