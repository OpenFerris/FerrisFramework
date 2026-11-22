//! File reference management

use crate::store::{DocRef, FileRef};
use crate::store::{FILE_REFS, ensure_file_refs_schema, row_to_file_ref, timestamp};
use chrono::{DateTime, Utc};
use ferris_core::{Error, FileError, Result, SerializationError, ValidationError, Value};
use ferris_db::{Db, Row, Stmt, sql::Column};
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

const FILE_VERSIONS: &str = "file_versions";

/// Every column the `file_versions` table declares.
///
/// `file_id` + `version` identify a version, and every read of this table is
/// by file, so the pair is the primary key.
fn file_versions_table() -> Vec<Column> {
    vec![
        Column::new("id", "VARCHAR(255)").not_null().primary_key(),
        Column::new("file_id", "VARCHAR(255)").not_null(),
        Column::new("version", "BIGINT").not_null(),
        Column::new("path", "VARCHAR(512)").not_null(),
        Column::new("size", "BIGINT").not_null(),
        Column::new("checksum", "VARCHAR(64)").not_null(),
        Column::new("created_by", "VARCHAR(255)").not_null(),
        Column::new("created_at", "BIGINT").not_null(),
        Column::new("comment", "TEXT"),
    ]
}

/// Build a [`FileVersion`] from a `file_versions` row.
fn row_to_file_version(row: &Row) -> FileVersion {
    FileVersion {
        id: row.str("id").unwrap_or_default(),
        file_id: row.str("file_id").unwrap_or_default(),
        version: row
            .opt_i64("version")
            .unwrap_or_default()
            .clamp(0, i64::from(u32::MAX)) as u32,
        path: row.str("path").unwrap_or_default(),
        size: row.opt_i64("size").unwrap_or_default().cast_unsigned(),
        checksum: row.str("checksum").unwrap_or_default(),
        created_by: row.str("created_by").unwrap_or_default(),
        created_at: timestamp(row.opt_i64("created_at")),
        comment: row.opt_str("comment"),
    }
}

/// File reference manager
pub struct FileReferenceManager {
    db: Db,
    store: Option<std::sync::Arc<crate::store::FileStore>>,
}

impl FileReferenceManager {
    pub fn new(db: Db) -> Self {
        Self { db, store: None }
    }

    /// Wire an object store so deleting the last reference also removes bytes.
    pub fn with_store(mut self, store: std::sync::Arc<crate::store::FileStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Create the tables the manager reads and writes (no-op when present).
    pub async fn init(&self) -> Result<()> {
        ensure_file_refs_schema(&self.db).await?;
        self.db
            .ensure_table(FILE_VERSIONS, &file_versions_table())
            .await?;
        // The version history is always read as a whole file's, newest first.
        self.db
            .ensure_index(
                "idx_file_versions_file_id",
                FILE_VERSIONS,
                &["file_id"],
                false,
            )
            .await
    }

    /// Attach file to document
    pub async fn attach_file(
        &self,
        file_id: &str,
        doctype: &str,
        docname: &str,
        fieldname: Option<&str>,
    ) -> Result<()> {
        // Verify file exists
        let file = self.get_file_ref(file_id).await?.ok_or_else(|| {
            Error::File(FileError::NotFound(format!("File {} not found", file_id)))
        })?;

        // Verify document exists (would check document store)

        // Update file reference
        let mut attached_to = file.attached_to;
        let doc_ref = DocRef {
            doctype: doctype.to_string(),
            name: docname.to_string(),
            fieldname: fieldname.map(|s| s.to_string()),
        };

        // Check if already attached
        if !attached_to.iter().any(|d| {
            d.doctype == doctype
                && d.name == docname
                && d.fieldname == fieldname.map(|s| s.to_string())
        }) {
            attached_to.push(doc_ref);

            // Update in database
            self.set_attached_to(file_id, &attached_to).await?;
        }

        Ok(())
    }

    /// Detach file from document
    pub async fn detach_file(
        &self,
        file_id: &str,
        doctype: &str,
        docname: &str,
        fieldname: Option<&str>,
    ) -> Result<()> {
        let mut file = self.get_file_ref(file_id).await?.ok_or_else(|| {
            Error::File(FileError::NotFound(format!("File {} not found", file_id)))
        })?;

        file.attached_to.retain(|d| {
            !(d.doctype == doctype
                && d.name == docname
                && d.fieldname == fieldname.map(|s| s.to_string()))
        });

        self.set_attached_to(file_id, &file.attached_to).await?;

        Ok(())
    }

    /// Persist a file's attachment list.
    async fn set_attached_to(&self, file_id: &str, attached_to: &[DocRef]) -> Result<()> {
        let attached_to = serde_json::to_string(attached_to)
            .map_err(|e| Error::Serialization(SerializationError::Json(e)))?;
        self.db
            .execute(&Stmt::new(
                format!("UPDATE {FILE_REFS} SET attached_to = ?, updated_at = ? WHERE id = ?"),
                vec![
                    Value::from(attached_to.as_str()),
                    Value::from(Utc::now().timestamp()),
                    Value::from(file_id),
                ],
            ))
            .await?;
        Ok(())
    }

    /// Get files attached to document.
    ///
    /// Uses a coarse `LIKE` pre-filter, then an exact match on the parsed
    /// attachments (the serialized JSON shape is not relied upon).
    pub async fn get_attached_files(&self, doctype: &str, docname: &str) -> Result<Vec<FileRef>> {
        let rows = self
            .db
            .fetch_all(&Stmt::new(
                format!("SELECT * FROM {FILE_REFS} WHERE attached_to LIKE ?"),
                vec![Value::from(format!("%{doctype}%"))],
            ))
            .await?;

        Ok(rows
            .iter()
            .map(row_to_file_ref)
            .filter(|f| {
                f.attached_to
                    .iter()
                    .any(|d| d.doctype == doctype && d.name == docname)
            })
            .collect())
    }

    /// Get files by document and field.
    ///
    /// The `LIKE` pre-filter only narrows on the DocType; the exact
    /// `(doctype, name, fieldname)` match happens on the parsed attachments.
    /// The old pre-filter required the literal substring `attached_to`, which
    /// the serialized `Vec<DocRef>` JSON never contains, so it matched zero
    /// rows and this method always returned an empty vector.
    pub async fn get_files_by_field(
        &self,
        doctype: &str,
        docname: &str,
        fieldname: &str,
    ) -> Result<Vec<FileRef>> {
        let rows = self
            .db
            .fetch_all(&Stmt::new(
                format!("SELECT * FROM {FILE_REFS} WHERE attached_to LIKE ?"),
                vec![Value::from(format!("%{doctype}%"))],
            ))
            .await?;

        let mut files = Vec::new();
        for file in rows.iter().map(row_to_file_ref) {
            // Filter by exact match
            if file.attached_to.iter().any(|d| {
                d.doctype == doctype
                    && d.name == docname
                    && d.fieldname.as_deref() == Some(fieldname)
            }) {
                files.push(file);
            }
        }

        Ok(files)
    }

    /// Get file reference
    pub async fn get_file_ref(&self, file_id: &str) -> Result<Option<FileRef>> {
        let row = self
            .db
            .fetch_optional(&Stmt::new(
                format!("SELECT * FROM {FILE_REFS} WHERE id = ?"),
                vec![Value::from(file_id)],
            ))
            .await?;

        Ok(row.as_ref().map(row_to_file_ref))
    }

    /// Delete file reference (and file if no more references)
    pub async fn delete_file_ref(&self, file_id: &str) -> Result<bool> {
        let file = self.get_file_ref(file_id).await?.ok_or_else(|| {
            Error::File(FileError::NotFound(format!("File {} not found", file_id)))
        })?;

        // Check if file has any attachments
        if !file.attached_to.is_empty() {
            return Err(Error::Validation(ValidationError::Invalid(
                "File is still attached to documents".to_string(),
            )));
        }

        // Delete from database
        let deleted = self
            .db
            .execute(&Stmt::new(
                format!("DELETE FROM {FILE_REFS} WHERE id = ?"),
                vec![Value::from(file_id)],
            ))
            .await?;

        // Remove the object bytes too (avoids orphaned storage).
        if !deleted.is_empty() {
            if let Some(store) = &self.store {
                if let Err(e) = store.delete(&file).await {
                    warn!("Failed to delete file object {}: {}", file_id, e);
                }
            }
        }

        Ok(!deleted.is_empty())
    }

    /// Get file versions
    pub async fn get_file_versions(&self, file_id: &str) -> Result<Vec<FileVersion>> {
        let rows = self
            .db
            .fetch_all(&Stmt::new(
                format!("SELECT * FROM {FILE_VERSIONS} WHERE file_id = ? ORDER BY version DESC"),
                vec![Value::from(file_id)],
            ))
            .await?;

        Ok(rows.iter().map(row_to_file_version).collect())
    }

    /// Create new file version
    pub async fn create_version(
        &self,
        file_id: &str,
        path: String,
        size: u64,
        checksum: String,
        created_by: String,
        comment: Option<String>,
    ) -> Result<FileVersion> {
        let current_versions = self.get_file_versions(file_id).await?;
        let next_version = current_versions.len() as u32 + 1;

        let version = FileVersion {
            id: Uuid::new_v4().to_string(),
            file_id: file_id.to_string(),
            version: next_version,
            path,
            size,
            checksum,
            created_by,
            created_at: Utc::now(),
            comment,
        };

        self.db
            .execute(&Stmt::new(
                format!(
                    "INSERT INTO {FILE_VERSIONS} (id, file_id, version, path, size, checksum, \
                     created_by, created_at, comment) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"
                ),
                vec![
                    Value::from(version.id.as_str()),
                    Value::from(version.file_id.as_str()),
                    Value::from(version.version),
                    Value::from(version.path.as_str()),
                    Value::from(version.size),
                    Value::from(version.checksum.as_str()),
                    Value::from(version.created_by.as_str()),
                    Value::from(version.created_at.timestamp()),
                    Value::from(version.comment.clone()),
                ],
            ))
            .await?;

        Ok(version)
    }

    /// Restore file version
    pub async fn restore_version(&self, file_id: &str, version: u32) -> Result<FileRef> {
        let versions = self.get_file_versions(file_id).await?;
        let version_data = versions
            .iter()
            .find(|v| v.version == version)
            .ok_or_else(|| {
                Error::File(FileError::NotFound(format!(
                    "Version {} not found",
                    version
                )))
            })?;

        // Update file reference to point to this version
        self.db
            .execute(&Stmt::new(
                format!(
                    "UPDATE {FILE_REFS} SET path = ?, size = ?, checksum = ?, updated_at = ? \
                     WHERE id = ?"
                ),
                vec![
                    Value::from(version_data.path.as_str()),
                    Value::from(version_data.size),
                    Value::from(version_data.checksum.as_str()),
                    Value::from(Utc::now().timestamp()),
                    Value::from(file_id),
                ],
            ))
            .await?;

        // Return updated file reference
        self.get_file_ref(file_id).await?.ok_or_else(|| {
            Error::File(FileError::NotFound(
                "File not found after restore".to_string(),
            ))
        })
    }
}

/// File version
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileVersion {
    pub id: String,
    pub file_id: String,
    pub version: u32,
    pub path: String,
    pub size: u64,
    pub checksum: String,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub comment: Option<String>,
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{FileStore, FileUpload, StorageBackend, StoreConfig, UploadOptions};

    #[test]
    fn test_file_version() {
        let version = FileVersion {
            id: "v1".to_string(),
            file_id: "f1".to_string(),
            version: 1,
            path: "/path/to/file".to_string(),
            size: 1024,
            checksum: String::new(),
            created_by: "user1".to_string(),
            created_at: Utc::now(),
            comment: Some("Initial version".to_string()),
        };

        assert_eq!(version.version, 1);
    }

    async fn db() -> Db {
        ferris_db::memory_db().await.unwrap()
    }

    /// A store and a reference manager sharing one in-memory database.
    async fn fixture() -> (Db, FileStore, FileReferenceManager) {
        let db = db().await;
        let store = FileStore::new(StoreConfig {
            backend: StorageBackend::Memory,
            local_path: None,
            ..Default::default()
        })
        .await
        .unwrap()
        .with_db(db.clone());
        store.init().await.unwrap();

        let manager = FileReferenceManager::new(db.clone());
        manager.init().await.unwrap();
        (db, store, manager)
    }

    async fn upload(store: &FileStore, fieldname: &str) -> FileRef {
        store
            .upload(
                FileUpload {
                    filename: "invoice.pdf".to_string(),
                    content_type: "application/pdf".to_string(),
                    content: b"%PDF-1.4".to_vec(),
                    size: 8,
                    checksum: String::new(),
                },
                UploadOptions {
                    user_id: "user@example.com".to_string(),
                    attached_to: vec![DocRef {
                        doctype: "Sales Invoice".to_string(),
                        name: "SINV-0001".to_string(),
                        fieldname: Some(fieldname.to_string()),
                    }],
                    ..Default::default()
                },
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn attaching_and_detaching_updates_the_persisted_reference() {
        let (_, store, manager) = fixture().await;
        let file_ref = upload(&store, "attachments").await;

        manager
            .attach_file(
                &file_ref.id,
                "Sales Invoice",
                "SINV-0002",
                Some("attachments"),
            )
            .await
            .unwrap();

        // The attachment list is read back from the database rather than from
        // the copy the caller holds.
        let attached = manager.get_file_ref(&file_ref.id).await.unwrap().unwrap();
        assert_eq!(attached.attached_to.len(), 2);

        let attached = manager
            .get_attached_files("Sales Invoice", "SINV-0002")
            .await
            .unwrap();
        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0].id, file_ref.id);

        manager
            .detach_file(
                &file_ref.id,
                "Sales Invoice",
                "SINV-0002",
                Some("attachments"),
            )
            .await
            .unwrap();
        let attached = manager.get_file_ref(&file_ref.id).await.unwrap().unwrap();
        assert_eq!(attached.attached_to.len(), 1);
        assert!(
            manager
                .get_attached_files("Sales Invoice", "SINV-0002")
                .await
                .unwrap()
                .is_empty()
        );
        // The original attachment survived the detach.
        assert_eq!(
            manager
                .get_attached_files("Sales Invoice", "SINV-0001")
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn a_version_can_be_created_and_restored() {
        let (_, store, manager) = fixture().await;
        let file_ref = upload(&store, "attachments").await;

        manager
            .create_version(
                &file_ref.id,
                "uploads/old.pdf".to_string(),
                8,
                "old-checksum".to_string(),
                "user@example.com".to_string(),
                Some("Previous revision".to_string()),
            )
            .await
            .unwrap();

        let versions = manager.get_file_versions(&file_ref.id).await.unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, 1);
        assert_eq!(versions[0].comment.as_deref(), Some("Previous revision"));

        let restored = manager.restore_version(&file_ref.id, 1).await.unwrap();
        assert_eq!(restored.path, "uploads/old.pdf");
        assert_eq!(restored.checksum, "old-checksum");
        assert!(manager.restore_version(&file_ref.id, 9).await.is_err());
    }

    #[tokio::test]
    async fn an_attached_file_cannot_be_deleted() {
        let (_, store, manager) = fixture().await;
        let file_ref = upload(&store, "attachments").await;

        assert!(manager.delete_file_ref(&file_ref.id).await.is_err());

        manager
            .detach_file(
                &file_ref.id,
                "Sales Invoice",
                "SINV-0001",
                Some("attachments"),
            )
            .await
            .unwrap();
        assert!(manager.delete_file_ref(&file_ref.id).await.unwrap());
        assert!(manager.get_file_ref(&file_ref.id).await.unwrap().is_none());

        // The store sees the deletion too, since both share one database.
        assert!(
            store
                .list("uploads", 10, &crate::store::AccessContext::admin("test"))
                .await
                .unwrap()
                .is_empty()
        );
    }
}
