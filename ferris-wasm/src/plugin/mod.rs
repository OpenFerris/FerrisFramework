//! A loaded app plugin: the instance, the export calls and the selective WASI
//! linker that backs it.

mod call;
mod errors;
mod exports;
mod instance;
mod wasi;

pub(crate) use call::in_guest_call;
pub use exports::PLUGIN_ROUTE_ID;
pub use instance::Plugin;
