//! Persistence of `file_refs` rows.

use ferris_core::{Error, Result, SerializationError, Value};
use ferris_db::{
    Db, Stmt,
    sql::{UpsertBuilder, UpsertSet},
};

use super::file_store::FileStore;
use super::schema::FILE_REFS;
use super::types::FileRef;

impl FileStore {
    /// Insert the reference, or overwrite it in place when the id already
    /// exists and bump the version.
    ///
    /// The upsert is spelled through [`UpsertBuilder`] so MySQL gets its
    /// `ON DUPLICATE KEY UPDATE` rather than the Postgres-only
    /// `ON CONFLICT (id) DO UPDATE` that used to be hard-coded here.
    pub(super) async fn save_file_ref(&self, db: &Db, file_ref: &FileRef) -> Result<()> {
        let backend = serde_json::to_string(&file_ref.backend)
            .map_err(|e| Error::Serialization(SerializationError::Json(e)))?;
        let attached_to = serde_json::to_string(&file_ref.attached_to)
            .map_err(|e| Error::Serialization(SerializationError::Json(e)))?;
        let filename = Value::from(file_ref.filename.as_str());
        let content_type = Value::from(file_ref.content_type.as_str());
        let size = Value::from(file_ref.size);
        let checksum = Value::from(file_ref.checksum.as_str());
        let backend = Value::from(backend.as_str());
        let path = Value::from(file_ref.path.as_str());
        let is_private = Value::from(file_ref.is_private);
        let attached_to = Value::from(attached_to.as_str());
        let updated_at = Value::from(file_ref.updated_at.timestamp());

        let upsert = UpsertBuilder::new(db.dialect(), FILE_REFS)
            .values(vec![
                ("id", Value::from(file_ref.id.as_str())),
                ("filename", filename.clone()),
                ("content_type", content_type.clone()),
                ("size", size.clone()),
                ("checksum", checksum.clone()),
                ("backend", backend.clone()),
                ("path", path.clone()),
                ("is_private", is_private.clone()),
                ("attached_to", attached_to.clone()),
                ("created_by", Value::from(file_ref.created_by.as_str())),
                ("created_at", Value::from(file_ref.created_at.timestamp())),
                ("updated_at", updated_at.clone()),
                ("version", Value::from(file_ref.version)),
            ])
            .conflict_on(["id"])
            .on_update("filename", UpsertSet::Value(filename))
            .on_update("content_type", UpsertSet::Value(content_type))
            .on_update("size", UpsertSet::Value(size))
            .on_update("checksum", UpsertSet::Value(checksum))
            .on_update("backend", UpsertSet::Value(backend))
            .on_update("path", UpsertSet::Value(path))
            .on_update("is_private", UpsertSet::Value(is_private))
            .on_update("attached_to", UpsertSet::Value(attached_to))
            .on_update("updated_at", UpsertSet::Value(updated_at))
            .on_update("version", UpsertSet::Increment(Value::from(1)))
            .build()?;
        db.upsert(upsert).await.map(|_| ())
    }

    pub(super) async fn delete_file_ref(&self, db: &Db, id: &str) -> Result<()> {
        db.execute(&Stmt::new(
            format!("DELETE FROM {FILE_REFS} WHERE id = ?"),
            vec![Value::from(id)],
        ))
        .await?;
        Ok(())
    }
}
