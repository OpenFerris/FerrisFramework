//! Upload, download, listing and deletion of file bytes, with ACL checks.

use ferris_core::{AuthzError, Error, FileError, Result, ValidationError, Value};
use ferris_db::Stmt;
use object_store::path::Path;
use tracing::info;
use uuid::Uuid;

use super::file_store::FileStore;
use super::schema::{FILE_REFS, escape_like_pattern, row_to_file_ref, sha256_hex};
use super::types::{AccessContext, FileRef, FileUpload, UploadOptions};

/// Detect an image type from its magic bytes (returns `None` for non-images).
fn detect_image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        None
    }
}

impl FileStore {
    /// Upload file
    ///
    /// # Errors
    ///
    /// Returns a validation error when the declared size or checksum does not
    /// match the real bytes, or a storage error when the object store rejects
    /// the write.
    pub async fn upload(&self, file: FileUpload, options: UploadOptions) -> Result<FileRef> {
        // Validate file
        self.validate_file(&file, &options)?;

        // Never store a caller-supplied digest: compute it from the bytes that
        // are actually written. A non-empty declared checksum is an assertion
        // about those bytes, so a mismatch is an error, not a value to keep.
        let checksum = sha256_hex(&file.content);
        let declared = file.checksum.trim();
        if !declared.is_empty() && !declared.eq_ignore_ascii_case(&checksum) {
            return Err(Error::Validation(ValidationError::InvalidValue {
                field: "checksum".to_string(),
                message: "declared checksum does not match the uploaded bytes".to_string(),
            }));
        }

        // Generate unique ID
        let file_id = Uuid::new_v4().to_string();
        let ext = std::path::Path::new(&file.filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let path = format!(
            "{}/{}.{}",
            options.folder,
            file_id,
            if ext.is_empty() { "" } else { ext }
        );

        // Upload to object store
        let path_obj = Path::from(path.as_str());

        self.store
            .put(&path_obj, file.content.into())
            .await
            .map_err(|e| Error::File(FileError::Upload(format!("Failed to upload file: {}", e))))?;

        // Create file reference
        let file_ref = FileRef {
            id: file_id,
            filename: file.filename,
            content_type: file.content_type.clone(),
            size: file.size,
            checksum,
            backend: self.config.backend,
            path: path.clone(),
            is_private: options.private,
            attached_to: options.attached_to,
            created_by: options.user_id.clone(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            version: 1,
        };

        // Save to database
        if let Some(db) = &self.db {
            self.save_file_ref(db, &file_ref).await?;
        }

        info!("Uploaded file: {}", file_ref.id);
        Ok(file_ref)
    }

    /// Check that `ctx` may read `file_ref` without transferring any bytes.
    ///
    /// Metadata reads (`blobstore::metadata`) use this so they enforce exactly
    /// the same ACL as [`FileStore::download`].
    ///
    /// # Errors
    ///
    /// Returns an authorization error when `ctx` may not read the file.
    pub fn check_readable(&self, file_ref: &FileRef, ctx: &AccessContext) -> Result<()> {
        self.ensure_readable(file_ref, ctx)
    }

    /// Download file after enforcing the ACL.
    pub async fn download(&self, file_ref: &FileRef, ctx: &AccessContext) -> Result<Vec<u8>> {
        self.ensure_readable(file_ref, ctx)?;
        let path = Path::from(file_ref.path.as_str());
        let result = self.store.get(&path).await.map_err(|e| {
            Error::File(FileError::Storage(format!(
                "Failed to download file: {}",
                e
            )))
        })?;

        let bytes = result.bytes().await.map_err(|e| {
            Error::File(FileError::Storage(format!(
                "Failed to read file bytes: {}",
                e
            )))
        })?;

        Ok(bytes.to_vec())
    }

    /// Download a byte range of a file after enforcing the same ACL as
    /// [`FileStore::download`].
    ///
    /// Reads `length` bytes starting at 0-based `offset`; the range is clamped
    /// to the file's size, so an `offset` at or past the end yields an empty
    /// vector and an over-long `length` stops at EOF. A partial read needs no
    /// new storage semantics — the object store already serves ranges.
    ///
    /// # Errors
    ///
    /// Returns an authorization error when `ctx` may not read the file, or an
    /// internal error when the object store cannot read the range.
    pub async fn download_range(
        &self,
        file_ref: &FileRef,
        ctx: &AccessContext,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>> {
        self.ensure_readable(file_ref, ctx)?;
        if offset >= file_ref.size {
            return Ok(Vec::new());
        }
        let end = offset.saturating_add(length).min(file_ref.size);
        // `object_store` addresses ranges with `usize`; a blob larger than the
        // address space cannot be held in memory anyway, so a clamped window is
        // the right degradation.
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let stop = usize::try_from(end).unwrap_or(usize::MAX);
        let path = Path::from(file_ref.path.as_str());
        let bytes = self
            .store
            .get_range(&path, start..stop)
            .await
            .map_err(|e| {
                Error::File(FileError::Storage(format!(
                    "Failed to read file range: {e}"
                )))
            })?;
        Ok(bytes.to_vec())
    }

    /// Read the raw bytes at an internal storage path, **without** an ACL check.
    ///
    /// Only for callers that have already authorized the read by another means —
    /// notably a verified signed URL ([`crate::signed_url::SignedDownloads`]).
    /// Never pass a raw client-supplied value here: the id-to-path resolution
    /// belongs in [`crate::reference::FileReferenceManager`], which is what keeps
    /// a caller from naming an arbitrary object key.
    ///
    /// # Errors
    ///
    /// Returns an internal error when the object store cannot read the bytes.
    pub async fn read_bytes_unchecked(&self, storage_path: &str) -> Result<Vec<u8>> {
        let path = Path::from(storage_path);
        let result = self.store.get(&path).await.map_err(|e| {
            Error::File(FileError::Storage(format!("Failed to download file: {e}")))
        })?;
        let bytes = result.bytes().await.map_err(|e| {
            Error::File(FileError::Storage(format!(
                "Failed to read file bytes: {e}"
            )))
        })?;
        Ok(bytes.to_vec())
    }

    /// Delete file bytes and its `file_refs` row.
    ///
    /// **Low-level primitive: it performs no authorization and skips the
    /// "still attached to a document" guard.** Callers that accept a
    /// client-supplied id must go through [`FileReferenceManager::delete_file_ref`]
    /// (the attachment guard) after checking ownership/admin themselves — the
    /// `blobstore` host import does exactly that (V3-WASM-2). Calling this
    /// directly with an untrusted reference is a data-loss bug.
    pub async fn delete(&self, file_ref: &FileRef) -> Result<()> {
        let path = Path::from(file_ref.path.as_str());
        self.store.delete(&path).await.map_err(|e| {
            Error::File(FileError::Storage(format!("Failed to delete file: {}", e)))
        })?;

        // Delete from database
        if let Some(db) = &self.db {
            self.delete_file_ref(db, &file_ref.id).await?;
        }

        Ok(())
    }

    /// List files under `folder` that `ctx` may read.
    ///
    /// The folder is a prefix, so it is escaped before it becomes a `LIKE`
    /// pattern (`%`/`_` in a folder name must not widen the match) and the
    /// result is filtered through the same ACL as [`Self::download`]: listing
    /// must not expose another user's private filenames, sizes or paths.
    ///
    /// # Errors
    ///
    /// Returns the storage error of the query when the database is reachable
    /// but the statement fails.
    pub async fn list(
        &self,
        folder: &str,
        limit: usize,
        ctx: &AccessContext,
    ) -> Result<Vec<FileRef>> {
        let Some(db) = &self.db else {
            return Ok(Vec::new());
        };
        let rows = db
            .fetch_all(&Stmt::new(
                format!(
                    "SELECT * FROM {FILE_REFS} WHERE path LIKE ? ESCAPE '!' \
                     ORDER BY created_at DESC LIMIT ?"
                ),
                vec![
                    Value::from(format!("{}%", escape_like_pattern(folder))),
                    Value::from(limit),
                ],
            ))
            .await?;

        Ok(rows
            .iter()
            .map(row_to_file_ref)
            .filter(|file| file.can_read(ctx))
            .collect())
    }

    fn ensure_readable(&self, file_ref: &FileRef, ctx: &AccessContext) -> Result<()> {
        if !file_ref.can_read(ctx) {
            return Err(Error::Authz(AuthzError::PermissionDenied {
                action: "read".to_string(),
                resource: format!("file:{}", file_ref.id),
            }));
        }
        Ok(())
    }

    fn validate_file(&self, file: &FileUpload, _options: &UploadOptions) -> Result<()> {
        // Never trust the client-declared size. `max_file_size_mb = 0` means
        // "no cap": the upload is then bounded by the host's own resources (and
        // the HTTP body limit), not by a framework number.
        let real_size = file.content.len() as u64;
        if self.config.max_file_size_mb > 0 {
            let limit = self.config.max_file_size_mb.saturating_mul(1024 * 1024);
            if real_size > limit {
                return Err(Error::File(FileError::TooLarge {
                    size: real_size,
                    limit,
                }));
            }
        }
        if file.size != real_size {
            return Err(Error::Validation(ValidationError::InvalidValue {
                field: "size".to_string(),
                message: "Declared file size does not match the uploaded content".to_string(),
            }));
        }

        // Images must actually look like images (magic bytes), so arbitrary
        // content cannot masquerade as an image and be served/optimized.
        if file.content_type.starts_with("image/") && detect_image_type(&file.content).is_none() {
            return Err(Error::File(FileError::InvalidType(
                "Content does not match the declared image type".to_string(),
            )));
        }

        // Check extension
        if !self.config.allowed_extensions.is_empty() {
            let ext = std::path::Path::new(&file.filename)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            if !self
                .config
                .allowed_extensions
                .iter()
                .any(|e| e.to_lowercase() == ext)
            {
                return Err(Error::File(FileError::InvalidType(format!(
                    "File extension .{ext} not allowed"
                ))));
            }
        }

        Ok(())
    }
}
