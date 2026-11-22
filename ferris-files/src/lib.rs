//! Ferris Files - File storage and management

pub mod doctypes;
#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "module")]
pub mod module;
pub mod optimizer;
pub mod reference;
pub mod signed_url;
pub mod store;

#[cfg(feature = "module")]
pub use module::FilesModule;
pub use optimizer::{ImageOptimizer, OptimizationOptions};
pub use reference::{FileReferenceManager, FileVersion};
pub use signed_url::{
    DownloadedFile, FILES_ROUTE_PREFIX, FileUrlSigner, SignedDownloads, sign_path, verify_path,
};
pub use store::{
    AccessContext, FileRef, FileStore, FileUpload, StorageBackend, StoreConfig, UploadOptions,
};
