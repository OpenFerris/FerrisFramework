//! Signature, expiry, tenant and ACL regressions for signed URLs.

use std::sync::Arc;
use std::time::Duration;

use crate::reference::FileReferenceManager;
use crate::store::{AccessContext, FileRef, FileStore};
use ferris_core::FileError;

use super::*;

fn parse_query(url: &str) -> (i64, String) {
    let mut expires = 0i64;
    let mut signature = String::new();
    for pair in url.split_once('?').expect("query string").1.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            match key {
                "expires" => expires = value.parse().expect("expires"),
                "signature" => signature = value.to_string(),
                _ => {}
            }
        }
    }
    (expires, signature)
}

#[test]
fn a_signature_round_trips_and_is_bound_to_path_and_deadline() {
    let secret = b"top-secret";
    let expires = Utc::now().timestamp() + 60;

    let signature = sign_path(secret, "abc", expires).unwrap();
    assert!(verify_path(secret, "abc", expires, &signature));
    // A different file must not verify with the same tag.
    assert!(!verify_path(secret, "xyz", expires, &signature));
    // A pushed-out deadline must not verify either...
    assert!(!verify_path(secret, "abc", expires + 1, &signature));
    // ...nor a different secret.
    assert!(!verify_path(b"other", "abc", expires, &signature));
}

#[test]
fn a_flipped_signature_byte_is_refused() {
    let secret = b"top-secret";
    let expires = Utc::now().timestamp() + 60;
    let signature = sign_path(secret, "abc", expires).unwrap();

    let mut bytes = URL_SAFE_NO_PAD.decode(&signature).unwrap();
    bytes[0] ^= 0x01;
    let tampered = URL_SAFE_NO_PAD.encode(bytes);
    assert!(!verify_path(secret, "abc", expires, &tampered));
}

#[test]
fn a_leading_slash_is_normalised_before_signing() {
    let signer = FileUrlSigner::new(b"top-secret".to_vec(), Duration::from_secs(60));
    let url = signer.signed_url("/abc").unwrap();
    assert!(url.starts_with("/files/abc?expires="), "{url}");

    let (expires, signature) = parse_query(&url);
    // The verifier receives the id without the slash; it must agree with
    // what was signed.
    assert!(signer.verify("abc", expires, &signature));
}

#[test]
fn an_expired_signature_is_refused_even_when_well_formed() {
    let secret = b"top-secret";
    let expires = Utc::now().timestamp() - 1;
    let signature = sign_path(secret, "abc", expires).unwrap();
    assert!(!verify_path(secret, "abc", expires, &signature));
}

#[test]
fn signer_mints_a_url_that_verifies_and_rejects_tampering() {
    let signer = FileUrlSigner::new(b"top-secret".to_vec(), Duration::from_secs(60));
    assert!(signer.is_usable());

    let url = signer.signed_url("file-123").unwrap();
    assert!(url.starts_with("/files/file-123?expires="), "{url}");
    assert!(url.contains("&signature="), "{url}");

    let (expires, signature) = parse_query(&url);
    assert!(signer.verify("file-123", expires, &signature));
    assert!(!signer.verify("file-124", expires, &signature));
    assert!(!signer.verify("file-123", expires + 1, &signature));
}

/// SEC-24: the tenant is inside the signed message, so two tenants sharing
/// a secret cannot replay each other's links.
#[test]
fn a_tenant_scoped_url_does_not_verify_under_another_tenant() {
    let tenant_a =
        FileUrlSigner::new(b"top-secret".to_vec(), Duration::from_secs(60)).with_tenant("tenant-a");
    let tenant_b =
        FileUrlSigner::new(b"top-secret".to_vec(), Duration::from_secs(60)).with_tenant("tenant-b");

    let url = tenant_a.signed_url_for("file-1", false).unwrap();
    let (expires, signature) = parse_query(&url);
    assert!(tenant_a.verify_for("file-1", false, expires, &signature));
    assert!(!tenant_b.verify_for("file-1", false, expires, &signature));
}

/// SEC-24: the visibility is inside the signed message, so a public grant
/// cannot be used against a file that is now private.
#[test]
fn a_public_grant_does_not_verify_as_a_private_one() {
    let signer = FileUrlSigner::new(b"top-secret".to_vec(), Duration::from_secs(60));
    let url = signer.signed_url_for("file-1", false).unwrap();
    let (expires, signature) = parse_query(&url);
    assert!(signer.verify_for("file-1", false, expires, &signature));
    assert!(!signer.verify_for("file-1", true, expires, &signature));
}

#[test]
fn from_env_is_none_without_a_secret() {
    // No global env mutation: this test only asserts the blank-secret rule
    // through a directly-constructed signer.
    assert!(!FileUrlSigner::new(Vec::new(), Duration::from_secs(60)).is_usable());
    let blank = FileUrlSigner::new(b"".to_vec(), Duration::from_secs(60));
    assert!(!blank.verify("x", Utc::now().timestamp() + 60, "whatever"));
}

async fn service() -> (SignedDownloads, ferris_db::Db) {
    use crate::store::{StorageBackend, StoreConfig};

    let db = ferris_db::memory_db().await.unwrap();
    let store = Arc::new(
        FileStore::new(StoreConfig {
            backend: StorageBackend::Memory,
            local_path: None,
            ..Default::default()
        })
        .await
        .unwrap()
        .with_db(db.clone()),
    );
    store.init().await.unwrap();
    let manager = Arc::new(FileReferenceManager::new(db.clone()).with_store(store.clone()));
    manager.init().await.unwrap();
    let signer = FileUrlSigner::new(b"top-secret".to_vec(), Duration::from_secs(60));
    (SignedDownloads::new(store, manager, signer), db)
}

async fn upload(files: &SignedDownloads, private: bool, owner: &str) -> FileRef {
    use crate::store::{FileUpload, UploadOptions};

    let content = b"artifact bytes".to_vec();
    files
        .store()
        .upload(
            FileUpload {
                filename: "artifact.bin".to_string(),
                content_type: "application/octet-stream".to_string(),
                size: content.len() as u64,
                checksum: String::new(),
                content,
            },
            UploadOptions {
                folder: "blobstore".to_string(),
                private,
                user_id: owner.to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn fetch_serves_a_signed_blob_and_refuses_tampering() {
    let (downloads, _db) = service().await;
    let file_ref = upload(&downloads, false, "service").await;

    let url = downloads.signed_url(&file_ref.id).await.unwrap();
    let (expires, signature) = parse_query(&url);

    let served = downloads
        .fetch(&file_ref.id, expires, &signature, None)
        .await
        .unwrap();
    assert_eq!(served.filename, "artifact.bin");
    assert_eq!(served.content_type, "application/octet-stream");
    assert_eq!(served.bytes, b"artifact bytes");

    // A wrong signature, a retargeted id and a stale deadline all fail.
    assert!(
        downloads
            .fetch(&file_ref.id, expires, "not-a-signature", None)
            .await
            .is_err()
    );
    assert!(
        downloads
            .fetch("another-id", expires, &signature, None)
            .await
            .is_err()
    );
    assert!(
        downloads
            .fetch(&file_ref.id, expires - 100_000, &signature, None)
            .await
            .is_err()
    );

    // A validly signed but unknown id is a genuine not-found, not a 403.
    let ghost = downloads.signer().signed_url("missing").unwrap();
    let (ghost_expires, ghost_signature) = parse_query(&ghost);
    assert!(matches!(
        downloads
            .fetch("missing", ghost_expires, &ghost_signature, None)
            .await,
        Err(Error::File(FileError::NotFound(_)))
    ));
}

/// SEC-24: a private blob needs the signature **and** an ACL context that
/// may read it; the unchecked read is never used for it.
#[tokio::test]
async fn a_private_signed_url_requires_an_access_context() {
    let (downloads, _db) = service().await;
    let file_ref = upload(&downloads, true, "owner@example.com").await;

    let url = downloads.signed_url(&file_ref.id).await.unwrap();
    let (expires, signature) = parse_query(&url);

    // The signature alone is not enough.
    assert!(matches!(
        downloads
            .fetch(&file_ref.id, expires, &signature, None)
            .await,
        Err(Error::Authz(_))
    ));
    // Nor is another user's context.
    let other = AccessContext::new("other@example.com");
    assert!(
        downloads
            .fetch(&file_ref.id, expires, &signature, Some(&other))
            .await
            .is_err()
    );
    // The owner (or an admin) may read it.
    let owner = AccessContext::new("owner@example.com");
    let served = downloads
        .fetch(&file_ref.id, expires, &signature, Some(&owner))
        .await
        .unwrap();
    assert_eq!(served.bytes, b"artifact bytes");
}
