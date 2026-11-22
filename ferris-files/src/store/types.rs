//! Storage configuration, file metadata and the access context.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Storage backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    Local,
    S3,
    Azure,
    Gcs,
    Memory,
}

/// Store configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreConfig {
    pub backend: StorageBackend,
    pub local_path: Option<PathBuf>,
    pub s3: Option<S3Config>,
    pub azure: Option<AzureConfig>,
    pub gcs: Option<GcsConfig>,
    pub max_file_size_mb: u64,
    pub allowed_extensions: Vec<String>,
    pub image_optimization: bool,
    pub image_quality: u8,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            backend: StorageBackend::Local,
            local_path: Some(PathBuf::from("./files")),
            s3: None,
            azure: None,
            gcs: None,
            max_file_size_mb: 50,
            allowed_extensions: Vec::new(),
            image_optimization: true,
            image_quality: 85,
        }
    }
}

/// S3 configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Config {
    pub bucket: String,
    pub region: String,
    pub endpoint: Option<String>,
    pub access_key_id: String,
    pub secret_access_key: String,
    pub force_path_style: bool,
}

/// Azure configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AzureConfig {
    pub account: String,
    pub container: String,
    pub access_key: String,
}

/// GCS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcsConfig {
    pub bucket: String,
    pub credentials_path: PathBuf,
}

/// Upload file
#[derive(Debug, Clone)]
pub struct FileUpload {
    pub filename: String,
    pub content_type: String,
    pub content: Vec<u8>,
    pub size: u64,
    /// Optional expected digest of `content`. The store always computes the
    /// SHA-256 of the real bytes and persists that; a non-empty declared value
    /// that disagrees is rejected instead of being trusted.
    pub checksum: String,
}

/// File reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRef {
    pub id: String,
    pub filename: String,
    pub content_type: String,
    pub size: u64,
    pub checksum: String,
    pub backend: StorageBackend,
    pub path: String,
    pub is_private: bool,
    pub attached_to: Vec<DocRef>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: u32,
}

/// Document reference for file attachment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocRef {
    pub doctype: String,
    pub name: String,
    pub fieldname: Option<String>,
}

/// Caller context used to enforce file ACLs.
#[derive(Debug, Clone)]
pub struct AccessContext {
    pub user_id: String,
    pub is_admin: bool,
}

/// Roles that bypass the per-file ACL: the framework superuser and the seeded
/// desk administrator.
const FILE_ADMIN_ROLES: &[&str] = &[
    "Administrator",
    ferris_core::permissions::SYSTEM_MANAGER_ROLE,
];

impl AccessContext {
    pub fn new(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            is_admin: false,
        }
    }

    pub fn admin(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            is_admin: true,
        }
    }

    /// The context of a signed-in user: the identity comes from the session,
    /// never from the request body, and an administrator keeps the bypass the
    /// store already grants [`AccessContext::admin`].
    ///
    /// This is the one place the HTTP layer turns a [`ferris_core::User`] into
    /// an ACL context, so listing and downloading cannot drift apart (a file
    /// an administrator can read must also be one they can see in the list).
    #[must_use]
    pub fn for_user(user: &ferris_core::User) -> Self {
        Self {
            user_id: user.id.clone(),
            is_admin: user.has_any_role(FILE_ADMIN_ROLES),
        }
    }
}

impl FileRef {
    /// Whether `ctx` may read this file. Public files are readable by anyone;
    /// private files only by their owner or an administrator.
    pub fn can_read(&self, ctx: &AccessContext) -> bool {
        !self.is_private || ctx.is_admin || self.created_by == ctx.user_id
    }
}

/// Upload options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadOptions {
    pub folder: String,
    pub private: bool,
    pub optimize: bool,
    pub user_id: String,
    pub attached_to: Vec<DocRef>,
}

impl Default for UploadOptions {
    fn default() -> Self {
        Self {
            folder: "uploads".to_string(),
            private: false,
            optimize: true,
            user_id: String::new(),
            attached_to: Vec::new(),
        }
    }
}
