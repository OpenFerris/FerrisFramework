//! `[files]` section.

use ferris_core::StorageBackend;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Files backend configuration.
///
/// Mirrors [`ferris_core::FilesConfig`] closely enough to convert with
/// [`Self::to_core`]; the only difference is `local_path` as a `String`, which
/// is the shape the CLI (and `ferris tenant`) already used and keeps a
/// pre-existing config file byte-compatible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesConfig {
    #[serde(default = "default_storage_backend")]
    pub backend: StorageBackend,
    pub local_path: String,
    #[serde(default)]
    pub s3: Option<ferris_core::S3Config>,
    #[serde(default)]
    pub azure: Option<ferris_core::AzureConfig>,
    #[serde(default)]
    pub gcs: Option<ferris_core::GcsConfig>,
    #[serde(default)]
    pub cdn_url: Option<String>,
    pub max_file_size_mb: u64,
    #[serde(default)]
    pub allowed_extensions: Vec<String>,
    #[serde(default = "default_true")]
    pub image_optimization: bool,
    #[serde(default = "default_image_quality")]
    pub image_quality: u8,
}

impl FilesConfig {
    /// The core configuration the server consumes.
    #[must_use]
    pub fn to_core(&self) -> ferris_core::FilesConfig {
        ferris_core::FilesConfig {
            backend: self.backend,
            local_path: PathBuf::from(&self.local_path),
            s3: self.s3.clone(),
            azure: self.azure.clone(),
            gcs: self.gcs.clone(),
            cdn_url: self.cdn_url.clone(),
            max_file_size_mb: self.max_file_size_mb,
            allowed_extensions: self.allowed_extensions.clone(),
            image_optimization: self.image_optimization,
            image_quality: self.image_quality,
        }
    }
}

impl Default for FilesConfig {
    fn default() -> Self {
        Self {
            backend: StorageBackend::Local,
            local_path: "./files".to_string(),
            s3: None,
            azure: None,
            gcs: None,
            cdn_url: None,
            max_file_size_mb: 50,
            allowed_extensions: Vec::new(),
            image_optimization: true,
            image_quality: 85,
        }
    }
}

const fn default_storage_backend() -> StorageBackend {
    StorageBackend::Local
}

const fn default_true() -> bool {
    true
}

const fn default_image_quality() -> u8 {
    85
}
