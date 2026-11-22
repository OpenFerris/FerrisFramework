//! Empty marker traits the component linker requires for the data-only WIT
//! interfaces.
//!
//! `types` and `descriptor` carry records, enums and variants but no
//! functions; `bindgen!` still emits a `Host` trait per imported interface, so
//! the store type has to implement them to satisfy the link.

use crate::host::HostState;

impl crate::ferris::framework::types::Host for HostState {}

impl crate::ferris::framework::descriptor::Host for HostState {}
