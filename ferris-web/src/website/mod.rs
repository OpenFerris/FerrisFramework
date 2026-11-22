//! Website/CMS module.
//!
//! Mirrors Frappe's `Web Page` doctype and `Website Route` routing: pages are
//! registered by route, resolved case-insensitively, and exported as full HTML
//! with SEO meta tags and a generated `sitemap.xml`.
//!
//! Split by concern: [`page`] (registry, HTML and sitemap), [`blog`] (posts and
//! their store), [`form`] (public forms and submission validation) and
//! [`discussion`] (threaded comments).

mod blog;
mod discussion;
mod form;
mod page;
mod portal;

#[cfg(test)]
mod tests;

pub use blog::{Blog, BlogPost};
pub use discussion::{Comment, Discussion};
pub use form::{WebForm, WebFormField};
pub use page::{RenderedPage, WebPage, WebsiteRouter, normalize_route};
pub use portal::{PublicRoute, resolve_public_route};
