//! Real host implementation backed by the framework services.
//!
//! [`RealHostApi`] lives in `real.rs`; every [`HostApi`](crate::host::HostApi)
//! interface has its own file, so a service change touches one seam instead of
//! a monolith.

mod aggregate;
mod api_impl;
mod config;
mod context;
mod docstore;
mod http;
mod identity;
mod notification;
mod pki;
mod process;
mod pubsub;
mod query;
mod real;
mod scheduler;
mod schema;
mod state;
mod streams;
#[cfg(test)]
mod tests;

pub use real::RealHostApi;
pub use state::RealHostState;
