use super::schema::sha256_hex;
use super::*;

use ferris_core::{Error, ValidationError};
use ferris_db::Db;

#[test]
fn test_store_config() {
    let config = StoreConfig::default();
    assert_eq!(config.backend, StorageBackend::Local);
    assert_eq!(config.max_file_size_mb, 50);
}

#[test]
fn test_file_upload() {
    let upload = FileUpload {
        filename: "test.txt".to_string(),
        content_type: "text/plain".to_string(),
        content: b"Hello World".to_vec(),
        size: 11,
        checksum: String::new(),
    };

    assert_eq!(upload.filename, "test.txt");
}

async fn db() -> Db {
    ferris_db::memory_db().await.unwrap()
}

/// A store with an in-memory object backend and a real database behind it,
/// so the persistence path is what runs.
async fn store() -> FileStore {
    let store = FileStore::new(StoreConfig {
        backend: StorageBackend::Memory,
        local_path: None,
        ..Default::default()
    })
    .await
    .unwrap()
    .with_db(db().await);
    store.init().await.unwrap();
    store
}

fn upload(filename: &str) -> FileUpload {
    let content = b"Hello Ferris".to_vec();
    FileUpload {
        filename: filename.to_string(),
        content_type: "text/plain".to_string(),
        size: content.len() as u64,
        checksum: String::new(),
        content,
    }
}

#[tokio::test]
async fn an_uploaded_file_is_persisted_and_listed() {
    let store = store().await;
    let options = UploadOptions {
        user_id: "user@example.com".to_string(),
        ..Default::default()
    };

    let file_ref = store.upload(upload("notes.txt"), options).await.unwrap();
    assert_eq!(file_ref.filename, "notes.txt");

    let listed = store
        .list("uploads", 10, &AccessContext::new("user@example.com"))
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, file_ref.id);
    assert_eq!(listed[0].size, 12);
    assert_eq!(listed[0].backend, StorageBackend::Memory);
    assert_eq!(listed[0].created_by, "user@example.com");
    assert_eq!(listed[0].version, 1);

    // The bytes are still readable through the ACL path.
    let ctx = AccessContext::new("user@example.com");
    assert_eq!(
        store.download(&file_ref, &ctx).await.unwrap(),
        b"Hello Ferris"
    );
}

#[tokio::test]
async fn an_uploaded_file_stores_the_computed_checksum() {
    let store = store().await;
    let content = b"Hello Ferris".to_vec();
    let expected = sha256_hex(&content);

    let mut upload = upload("notes.txt");
    upload.content = content;
    upload.size = 12;
    // A declared digest that matches is accepted.
    upload.checksum = expected.clone();

    let file_ref = store
        .upload(upload, UploadOptions::default())
        .await
        .unwrap();
    assert_eq!(file_ref.checksum, expected);
    assert_eq!(file_ref.checksum.len(), 64);
}

#[tokio::test]
async fn a_mismatched_declared_checksum_is_rejected() {
    let store = store().await;
    let mut upload = upload("notes.txt");
    upload.checksum = "0".repeat(64);

    let err = store
        .upload(upload, UploadOptions::default())
        .await
        .expect_err("a digest that does not match the bytes must be rejected");
    assert!(
        matches!(
            err,
            Error::Validation(ValidationError::InvalidValue { ref field, .. })
                if field.as_str() == "checksum"
        ),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn a_private_file_is_only_readable_by_its_owner_or_an_admin() {
    let store = store().await;
    let options = UploadOptions {
        private: true,
        user_id: "user@example.com".to_string(),
        ..Default::default()
    };
    let file_ref = store.upload(upload("secret.txt"), options).await.unwrap();

    assert!(
        store
            .download(&file_ref, &AccessContext::new("user@example.com"))
            .await
            .is_ok()
    );
    assert!(
        store
            .download(&file_ref, &AccessContext::new("other@example.com"))
            .await
            .is_err()
    );
    assert!(
        store
            .download(&file_ref, &AccessContext::admin("other@example.com"))
            .await
            .is_ok()
    );

    // The private flag survives the round trip through the database.
    let listed = store
        .list("uploads", 10, &AccessContext::new("user@example.com"))
        .await
        .unwrap();
    assert!(listed[0].is_private);
}

#[tokio::test]
async fn list_hides_private_files_from_other_users() {
    let store = store().await;
    let options = UploadOptions {
        private: true,
        user_id: "owner@example.com".to_string(),
        ..Default::default()
    };
    store.upload(upload("secret.txt"), options).await.unwrap();

    let owner = store
        .list("uploads", 10, &AccessContext::new("owner@example.com"))
        .await
        .unwrap();
    assert_eq!(owner.len(), 1);

    let other = store
        .list("uploads", 10, &AccessContext::new("other@example.com"))
        .await
        .unwrap();
    assert!(other.is_empty(), "private metadata leaked to another user");
}

#[tokio::test]
async fn list_treats_the_folder_as_a_literal_prefix() {
    let store = store().await;
    store
        .upload(upload("notes.txt"), UploadOptions::default())
        .await
        .unwrap();

    let all = store
        .list("%", 10, &AccessContext::admin("admin@example.com"))
        .await
        .unwrap();
    assert!(all.is_empty(), "`%` must not act as a wildcard");
}

/// The upsert must resolve the conflict on the `id` primary key, which
/// only holds if the table declares one.
#[tokio::test]
async fn saving_the_same_id_updates_in_place_and_bumps_the_version() {
    let store = store().await;
    let mut file_ref = store
        .upload(upload("notes.txt"), UploadOptions::default())
        .await
        .unwrap();

    file_ref.filename = "renamed.txt".to_string();
    let db = store.db.as_ref().unwrap();
    store.save_file_ref(db, &file_ref).await.unwrap();

    let listed = store
        .list("uploads", 10, &AccessContext::new(""))
        .await
        .unwrap();
    assert_eq!(
        listed.len(),
        1,
        "the conflict must resolve to an update, not a second row"
    );
    assert_eq!(listed[0].filename, "renamed.txt");
    assert_eq!(listed[0].version, 2);
}

#[tokio::test]
async fn deleting_a_file_removes_its_reference_and_bytes() {
    let store = store().await;
    let file_ref = store
        .upload(upload("gone.txt"), UploadOptions::default())
        .await
        .unwrap();

    store.delete(&file_ref).await.unwrap();
    assert!(
        store
            .list("uploads", 10, &AccessContext::new(""))
            .await
            .unwrap()
            .is_empty()
    );
}
