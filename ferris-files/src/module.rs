//! The files feature as a native module.
//!
//! The `DocType` is always declarative. With the `http` feature the same module
//! also owns the Desk routes and the public signed download, and its `boot`
//! builds the shared store those handlers use.

use std::sync::{Arc, OnceLock};

use crate::SignedDownloads;

/// The file service and the routes that expose it.
#[derive(Default, Clone)]
pub struct FilesModule {
    pub(crate) signed: Arc<OnceLock<Option<Arc<SignedDownloads>>>>,
}
