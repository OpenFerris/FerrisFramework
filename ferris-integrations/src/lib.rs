//! Integrations for Ferris: the DocTypes and the Desk workspace that configure
//! how a Ferris install talks to the outside world.
//!
//! # Two layers
//!
//! * **Declarations** — [`doctypes`] declares the DocType metas and
//!   [`workspaces`] the Integrations sidebar section. Neither touches the
//!   database or the network; the host registers both at boot
//!. This is all that compiles by default.
//! * **Runtime** — [`runtime`] (feature `runtime`) implements what those
//!   DocTypes describe: document-event hooks that fire webhooks, run automation
//!   flows, apply assignment rules and track milestones; scheduled jobs for
//!   auto repeat, reminders and auto email reports; push delivery; and the
//!   OAuth/Google/LDAP adapters. `ferris-server` turns the feature on with its
//!   runtime roles.
//!
//! Keeping the runtime behind the `runtime` feature means the Desk always knows
//! what an integration *is* even on a build that implements none of it, and a
//! build that does implement it pays exactly the dependency cost it uses.
#![allow(clippy::doc_markdown)]

pub mod doctypes;
#[cfg(feature = "module")]
pub mod module;
pub mod workspaces;

#[cfg(feature = "runtime")]
pub mod runtime;

#[cfg(feature = "module")]
pub use module::IntegrationsModule;
