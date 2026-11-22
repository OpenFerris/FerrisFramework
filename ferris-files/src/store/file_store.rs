//! The [`FileStore`] handle and its lifecycle (construction, schema, migration).

use ferris_core::{Error, FileError, FilesConfig, Result, Value};
use ferris_db::{Db, Stmt};
use object_store::ObjectStore;
use std::path::PathBuf;
use std::sync::Arc;

use super::backend::create_object_store;
use super::schema::{FILE_REFS, ensure_file_refs_schema};
use super::types::{AzureConfig, GcsConfig, S3Config, StorageBackend, StoreConfig};

/// File store
pub struct FileStore {
    pub(super) config: StoreConfig,
    pub(super) store: Arc<dyn ObjectStore>,
    pub(super) db: Option<Db>,
    /// `[files].cdn_url`: the public origin attachments are served from. The
    /// route behind it still enforces the ACL; only the origin changes.
    cdn_url: Option<String>,
}

/// Project the operator's `[files]` section onto the store's configuration.
///
/// The two structs describe the same storage at different layers: `FilesConfig`
/// is what `ferris.toml` carries and `StoreConfig` is what the store consumes.
/// The boot used to open an environment-only store, so the backend, root,
/// limits, extension allow-list, optimization quality and CDN prefix in
/// `[files]` had no effect (FILE-04).
#[must_use]
pub fn store_config_from_files(files: &FilesConfig) -> StoreConfig {
    StoreConfig {
        backend: match files.backend {
            ferris_core::StorageBackend::Local => StorageBackend::Local,
            ferris_core::StorageBackend::S3 => StorageBackend::S3,
            ferris_core::StorageBackend::Azure => StorageBackend::Azure,
            ferris_core::StorageBackend::Gcs => StorageBackend::Gcs,
            ferris_core::StorageBackend::Memory => StorageBackend::Memory,
        },
        local_path: Some(files.local_path.clone()),
        s3: files.s3.as_ref().map(|s3| S3Config {
            bucket: s3.bucket.clone(),
            region: s3.region.clone(),
            endpoint: s3.endpoint.clone(),
            access_key_id: s3.access_key_id.clone(),
            secret_access_key: s3.secret_access_key.clone(),
            force_path_style: s3.force_path_style,
        }),
        azure: files.azure.as_ref().map(|azure| AzureConfig {
            account: azure.account.clone(),
            container: azure.container.clone(),
            access_key: azure.access_key.clone(),
        }),
        gcs: files.gcs.as_ref().map(|gcs| GcsConfig {
            bucket: gcs.bucket.clone(),
            credentials_path: gcs.credentials_path.clone(),
        }),
        max_file_size_mb: files.max_file_size_mb,
        allowed_extensions: files.allowed_extensions.clone(),
        image_optimization: files.image_optimization,
        image_quality: files.image_quality,
    }
}

impl FileStore {
    pub async fn new(config: StoreConfig) -> Result<Self> {
        let store = create_object_store(&config).await?;

        Ok(Self {
            config,
            store: Arc::new(store),
            db: None,
            cdn_url: None,
        })
    }

    /// Open the store the `[files]` section describes.
    ///
    /// A local backend creates its root first: `LocalFileSystem` canonicalizes
    /// the prefix and requires it to exist. Remote backends are built from the
    /// configured credentials and never touch the filesystem here.
    ///
    /// # Errors
    ///
    /// Returns an internal error when the local root cannot be created or the
    /// configured backend cannot be constructed.
    // Same crate-wide `Result` as every store constructor; boxing only this one
    // would change the call sites for no benefit.
    #[allow(clippy::result_large_err)]
    pub async fn open_configured(files: &FilesConfig) -> Result<Self> {
        let config = store_config_from_files(files);
        if config.backend == StorageBackend::Local
            && let Some(root) = &config.local_path
        {
            std::fs::create_dir_all(root).map_err(|e| {
                Error::File(FileError::Storage(format!(
                    "cannot create file store root {}: {e}",
                    root.display()
                )))
            })?;
        }
        let store = Self::new(config).await?;
        Ok(store.with_cdn_url(files.cdn_url.clone()))
    }

    /// Open the framework's shared object store.
    ///
    /// The root is `FERRIS_FILES_DIR` (default `<temp>/ferris/blobstore`) — the
    /// exact location the `blobstore` host imports write to. Serving signed
    /// downloads from the same store is what guarantees the web tier reads what
    /// a plugin uploaded, without either side re-deriving the path. A deployment
    /// with `[files]` uses [`FileStore::open_configured`] instead.
    ///
    /// # Errors
    ///
    /// Returns an internal error when the root cannot be created or the backing
    /// object store cannot be constructed.
    pub async fn open_shared() -> Result<Self> {
        let root = std::env::var_os("FERRIS_FILES_DIR").map_or_else(
            || std::env::temp_dir().join("ferris/blobstore"),
            PathBuf::from,
        );
        // `LocalFileSystem::new_with_prefix` canonicalizes its root, which
        // requires the directory to already exist (or the nearest ancestor to
        // exist), so create it up front.
        std::fs::create_dir_all(&root).map_err(|e| {
            Error::File(FileError::Storage(format!(
                "cannot create file store root {}: {e}",
                root.display()
            )))
        })?;
        Self::new(StoreConfig {
            local_path: Some(root),
            ..Default::default()
        })
        .await
    }

    /// Apply the configured CDN origin (empty/blank means "no CDN").
    #[must_use]
    pub fn with_cdn_url(mut self, cdn_url: Option<String>) -> Self {
        self.cdn_url = cdn_url.filter(|url| !url.trim().is_empty());
        self
    }

    /// The public origin attachments are served from, when configured.
    #[must_use]
    pub fn cdn_url(&self) -> Option<&str> {
        self.cdn_url.as_deref()
    }

    /// The URL a browser uses for `file_id`.
    ///
    /// With `[files].cdn_url` this is `<cdn>/api/files/<id>`; without it, the
    /// Desk's own route. The handler behind both enforces the same ACL.
    #[must_use]
    pub fn public_url(&self, file_id: &str) -> String {
        let path = format!("/api/files/{file_id}");
        self.cdn_url.as_ref().map_or_else(
            || path.clone(),
            |cdn| format!("{}{path}", cdn.trim_end_matches('/')),
        )
    }

    pub fn with_db(mut self, db: Db) -> Self {
        self.db = Some(db);
        self
    }

    /// Create the tables the store writes to (no-op without a database).
    pub async fn init(&self) -> Result<()> {
        let Some(db) = &self.db else {
            return Ok(());
        };
        ensure_file_refs_schema(db).await
    }

    /// Close the public exposure of blobs written before `blobstore::put`
    /// stored them privately (H6).
    ///
    /// The plugin namespace writes every blob under the `blobstore/` folder, so
    /// the prefix names exactly the rows this migration owns; a user-uploaded
    /// file is never touched. Idempotent: after the first run the predicate
    /// matches nothing, which is why the host can call it on every boot.
    ///
    /// Returns the number of rows it flipped to private.
    ///
    /// # Errors
    ///
    /// Returns the storage error of the update when the database is reachable
    /// but the statement fails.
    pub async fn privatize_blobstore_files(&self) -> Result<u64> {
        let Some(db) = &self.db else {
            return Ok(0);
        };
        let affected = db
            .execute(&Stmt::new(
                format!(
                    "UPDATE {FILE_REFS} SET is_private = 1 \
                     WHERE is_private = 0 AND path LIKE ?"
                ),
                vec![Value::from("blobstore/%")],
            ))
            .await?;
        Ok(affected.rows())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("ferris-files-test-{}", uuid::Uuid::new_v4()))
    }

    /// FILE-04: `[files]` decides the effective root and the upload limits.
    #[tokio::test]
    async fn the_files_config_drives_the_local_root_and_limits() {
        let root = temp_root();
        let files = FilesConfig {
            local_path: root.clone(),
            max_file_size_mb: 2,
            allowed_extensions: vec!["txt".to_string()],
            image_optimization: false,
            image_quality: 60,
            cdn_url: Some("https://cdn.example/".to_string()),
            ..FilesConfig::default()
        };

        let store = FileStore::open_configured(&files).await.expect("store");
        assert_eq!(store.config.backend, StorageBackend::Local);
        assert_eq!(store.config.local_path.as_deref(), Some(root.as_path()));
        assert_eq!(store.config.max_file_size_mb, 2);
        assert_eq!(store.config.allowed_extensions, vec!["txt"]);
        assert!(!store.config.image_optimization);
        assert_eq!(store.config.image_quality, 60);
        assert!(root.is_dir(), "the configured root must exist");
        assert_eq!(
            store.public_url("f1"),
            "https://cdn.example/api/files/f1",
            "cdn_url changes only the origin of the attachment URL"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A remote backend (and its credentials) reaches the store, so S3/Azure/
    /// GCS stop being unreachable from the boot path.
    #[test]
    fn a_remote_backend_and_its_credentials_are_projected() {
        let files = FilesConfig {
            backend: ferris_core::StorageBackend::S3,
            s3: Some(ferris_core::S3Config {
                bucket: "bucket".to_string(),
                region: "eu-west-1".to_string(),
                endpoint: Some("https://s3.example".to_string()),
                access_key_id: "id".to_string(),
                secret_access_key: "secret".to_string(),
                force_path_style: true,
            }),
            ..FilesConfig::default()
        };

        let config = store_config_from_files(&files);
        assert_eq!(config.backend, StorageBackend::S3);
        let s3 = config.s3.expect("s3 configuration");
        assert_eq!(s3.bucket, "bucket");
        assert_eq!(s3.endpoint.as_deref(), Some("https://s3.example"));
        assert!(s3.force_path_style);
    }
}
