//! File storage abstraction
// Simplified version for compilation

mod backend;
mod file_store;
mod persist;
mod schema;
mod transfer;
mod types;

pub use file_store::FileStore;
pub use types::{
    AccessContext, AzureConfig, DocRef, FileRef, FileUpload, GcsConfig, S3Config, StorageBackend,
    StoreConfig, UploadOptions,
};

pub(crate) use schema::{FILE_REFS, ensure_file_refs_schema, row_to_file_ref, timestamp};

#[cfg(test)]
mod tests;
