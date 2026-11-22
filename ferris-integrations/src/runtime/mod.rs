//! Runtime implementation for the integration DocTypes. Only compiled with the `runtime` feature.
//!
//! # What lives where
//!
//! * [`state`] — the shared [`IntegrationsContext`]: the `Db`, the HTTP client,
//!   the [`Mailer`] sink and the service user every engine operation runs as.
//! * [`store`] — thin access helpers over [`ferris_document::DocumentEngine`]
//!   for the framework's own DocTypes.
//! * [`webhook`], [`automation`], [`assignment`], [`milestone`] — the
//!   document-event behaviours, mounted by [`hooks::install`]:
//!   `after_insert`/`on_update`/`on_submit`/`on_cancel`/`on_trash`.
//! * [`auto_repeat`], [`reminder`], [`auto_email_report`] — the scheduled
//!   jobs, each with a pure due-date core and a `run_due` that performs the
//!   work. [`jobs`] exposes them as `JobHandler`s.
//! * [`push`] — delivers `Push Notification` rows as in-app notifications.
//! * [`oauth`], [`google`], [`ldap`] — the external adapters.
//!
//! # Wiring
//!
//! Whoever hosts the runtime calls [`hooks::install`] once at boot and registers
//! the job handlers ([`jobs::register_handlers`]) on the app `JobDispatcher`.
//! `ferris-server` does both in its worker/scheduler roles.
#![allow(clippy::doc_markdown)]
// The shared `ferris_core::Error` enum exceeds clippy's default 256-byte
// `result_large_err` threshold, so every crate in this workspace emits the lint
// on `Result<_, Error>` returns (see `ferris-core`, `ferris-document`,
// `ferris-auth`). Boxing the variants of a crate-shared error type is out of
// scope for the integrations, and the lint adds no information here — every
// `Result` in this module is exactly `ferris_core::Result`.
#![allow(clippy::result_large_err)]

pub mod assignment;
pub mod auto_email_report;
pub mod auto_repeat;
pub mod automation;
pub mod condition;
pub mod google;
pub mod hooks;
pub mod http;
pub mod jobs;
pub mod ldap;
pub mod milestone;
pub mod names;
pub mod oauth;
pub mod push;
pub mod reminder;
pub mod state;
pub mod store;
pub mod template;
pub mod webhook;

// Anchor the tokio dependency for `unused_crate_dependencies`: the library
// itself is host-driven (no spawning of its own), but the feature's async
// tests — `#[tokio::test]` in these modules — run on its reactor. The feature
// gate keeps tokio out of feature-less builds entirely.
#[cfg(feature = "runtime")]
use tokio as _;

#[cfg(test)]
mod testing;

#[cfg(all(test, feature = "runtime"))]
mod flows_tests;

pub use automation::evaluate_condition;
pub use hooks::install;
pub use jobs::register_handlers;
pub use state::{EmailMessage, IntegrationsContext, Mailer, NullMailer};
