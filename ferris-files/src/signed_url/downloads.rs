//! Resolve a signed URL against the shared store, enforcing the blob's ACL.

use std::sync::Arc;
use std::time::Duration;

use ferris_core::{AuthzError, Error, FileError, Result};

use super::{FileUrlSigner, normalize_file_id};
use crate::reference::FileReferenceManager;
use crate::store::{AccessContext, FileRef, FileStore};

/// A file resolved through a verified signed URL.
#[derive(Debug, Clone)]
pub struct DownloadedFile {
    pub id: String,
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// Resolves signed URLs against the shared store.
///
/// This is the read half of the `blobstore` contract: a plugin mints a URL via
/// [`FileUrlSigner`], and the web tier (or any HTTP front) serves it through
/// [`SignedDownloads::fetch`]. A public blob needs only the signature; a
/// private one also needs the caller's [`AccessContext`].
pub struct SignedDownloads {
    store: Arc<FileStore>,
    manager: Arc<FileReferenceManager>,
    signer: FileUrlSigner,
}

impl SignedDownloads {
    #[must_use]
    pub const fn new(
        store: Arc<FileStore>,
        manager: Arc<FileReferenceManager>,
        signer: FileUrlSigner,
    ) -> Self {
        Self {
            store,
            manager,
            signer,
        }
    }

    /// The signer, so callers that already authorized a share can mint a URL.
    #[must_use]
    pub const fn signer(&self) -> &FileUrlSigner {
        &self.signer
    }

    /// The shared store behind this service.
    ///
    /// The Desk's own file endpoints (`/api/files`) list, upload and download
    /// through it with a session `AccessContext`; a signed URL is not the only
    /// way to reach a blob.
    #[must_use]
    pub fn store(&self) -> &FileStore {
        &self.store
    }

    /// The file-reference manager (resolve an id to its `FileRef`).
    #[must_use]
    pub fn manager(&self) -> &FileReferenceManager {
        &self.manager
    }

    async fn file_ref(&self, file_id: &str) -> Result<FileRef> {
        let file_id = normalize_file_id(file_id);
        self.manager
            .get_file_ref(file_id)
            .await?
            .ok_or_else(|| Error::File(FileError::NotFound(format!("file {file_id} not found"))))
    }

    /// Mint a temporary URL for a file that exists in the store.
    ///
    /// Resolving the row first is what lets the signature bind the stored
    /// visibility and the signer's tenant (SEC-24).
    ///
    /// # Errors
    ///
    /// Not-found when the file does not exist; propagates the HMAC error from
    /// [`FileUrlSigner::signed_url_for_with_ttl`].
    pub async fn signed_url(&self, file_id: &str) -> Result<String> {
        self.signed_url_with_ttl(file_id, self.signer.default_ttl)
            .await
    }

    /// Mint a temporary URL for a file that exists in the store, valid for
    /// `ttl`.
    ///
    /// # Errors
    ///
    /// See [`SignedDownloads::signed_url`].
    pub async fn signed_url_with_ttl(&self, file_id: &str, ttl: Duration) -> Result<String> {
        let file_ref = self.file_ref(file_id).await?;
        self.signer
            .signed_url_for_with_ttl(&file_ref.id, file_ref.is_private, ttl)
    }

    /// Verify the signature, resolve the file and read its bytes.
    ///
    /// The row is resolved **before** the signature is checked so the claims
    /// can include the stored visibility: a grant minted while the file was
    /// public stops verifying once it becomes private. For a public file the
    /// signature is the authorization; a private file additionally requires
    /// `ctx` and goes through the ACL-checked read.
    ///
    /// # Errors
    ///
    /// - permission denied for a forged/expired signature, or a private file
    ///   without a readable `ctx`;
    /// - not-found when the id no longer exists;
    /// - internal when the object store cannot read the bytes.
    pub async fn fetch(
        &self,
        file_id: &str,
        expires: i64,
        signature: &str,
        ctx: Option<&AccessContext>,
    ) -> Result<DownloadedFile> {
        let file_ref = self.file_ref(file_id).await?;
        if !self
            .signer
            .verify_for(&file_ref.id, file_ref.is_private, expires, signature)
        {
            return Err(permission_denied(&file_ref.id));
        }
        let bytes = if file_ref.is_private {
            let ctx = ctx.ok_or_else(|| permission_denied(&file_ref.id))?;
            // Never `read_bytes_unchecked` for a private blob: the ACL is the
            // second half of the authorization, not an optional extra.
            self.store.download(&file_ref, ctx).await?
        } else {
            self.store.read_bytes_unchecked(&file_ref.path).await?
        };
        Ok(DownloadedFile {
            id: file_ref.id,
            filename: file_ref.filename,
            content_type: file_ref.content_type,
            bytes,
        })
    }
}

fn permission_denied(file_id: &str) -> Error {
    Error::Authz(AuthzError::PermissionDenied {
        action: "read".to_string(),
        resource: format!("file:{file_id}"),
    })
}
