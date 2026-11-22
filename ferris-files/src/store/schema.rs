//! The `file_refs` table, row decoding and small shared helpers.

use chrono::{DateTime, Utc};
use ferris_core::Result;
use ferris_db::{Db, Row as DbRow, sql::Column};
use sha2::{Digest, Sha256};

use super::types::{FileRef, StorageBackend};

// The items below are declared `pub` inside this private module on purpose:
// they are shared with sibling modules of `store` (notably `reference`), and
// `pub(crate)` here would trip `clippy::redundant_pub_crate`. Their effective
// visibility is still crate-only, through the re-exports in `store/mod.rs`.
pub const FILE_REFS: &str = "file_refs";

/// Every column the `file_refs` table declares, in one place.
///
/// `is_private` is `INTEGER` rather than `BOOLEAN` because SQLite applies
/// INTEGER affinity to a `BOOLEAN` column and MySQL has no true boolean;
/// [`DbRow::opt_bool`] reads back whichever shape the backend produced. The
/// timestamps are `BIGINT` epoch seconds, so they order numerically on every
/// backend instead of lexically as text.
///
/// `id` is the primary key because `save_file_ref` upserts on it: a conflict
/// target must name a unique constraint.
fn file_refs_table() -> Vec<Column> {
    vec![
        Column::new("id", "VARCHAR(255)").not_null().primary_key(),
        Column::new("filename", "VARCHAR(255)").not_null(),
        Column::new("content_type", "VARCHAR(255)").not_null(),
        Column::new("size", "BIGINT").not_null(),
        Column::new("checksum", "VARCHAR(64)").not_null(),
        Column::new("backend", "TEXT").not_null(),
        Column::new("path", "VARCHAR(512)").not_null(),
        Column::new("is_private", "INTEGER").not_null().default("0"),
        Column::new("attached_to", "TEXT").not_null(),
        Column::new("created_by", "VARCHAR(255)").not_null(),
        Column::new("created_at", "BIGINT").not_null(),
        Column::new("updated_at", "BIGINT").not_null(),
        Column::new("version", "BIGINT").not_null().default("1"),
    ]
}

/// Create the `file_refs` table and its indexes (no-op when already present).
pub async fn ensure_file_refs_schema(db: &Db) -> Result<()> {
    db.ensure_table(FILE_REFS, &file_refs_table()).await?;
    // Every read is by id except `list`, which scans a folder prefix; the
    // attachment lookups match inside a JSON blob and cannot use an index.
    db.ensure_index("idx_file_refs_path", FILE_REFS, &["path"], false)
        .await
}

/// Build a [`FileRef`] from a `file_refs` row.
///
/// Columns are read through [`DbRow`] rather than `row.get`, so a NULL that a
/// backend produced for an absent optional value decodes as a default instead
/// of panicking.
pub fn row_to_file_ref(row: &DbRow) -> FileRef {
    FileRef {
        id: row.str("id").unwrap_or_default(),
        filename: row.str("filename").unwrap_or_default(),
        content_type: row.str("content_type").unwrap_or_default(),
        size: row.opt_i64("size").unwrap_or_default().cast_unsigned(),
        checksum: row.str("checksum").unwrap_or_default(),
        backend: serde_json::from_str(&row.str("backend").unwrap_or_default())
            .unwrap_or(StorageBackend::Local),
        path: row.str("path").unwrap_or_default(),
        is_private: row.opt_bool("is_private").unwrap_or(false),
        attached_to: serde_json::from_str(&row.str("attached_to").unwrap_or_default())
            .unwrap_or_default(),
        created_by: row.str("created_by").unwrap_or_default(),
        created_at: timestamp(row.opt_i64("created_at")),
        updated_at: timestamp(row.opt_i64("updated_at")),
        version: row
            .opt_i64("version")
            .unwrap_or_default()
            .clamp(0, i64::from(u32::MAX)) as u32,
    }
}

/// An epoch-seconds column, falling back to now when it is absent or unusable.
pub fn timestamp(value: Option<i64>) -> DateTime<Utc> {
    value
        .and_then(|secs| DateTime::from_timestamp(secs, 0))
        .unwrap_or_else(Utc::now)
}

/// Escape a literal for a SQL `LIKE` pattern using `!` as the escape char.
///
/// `!` rather than `\` because MySQL rejects `ESCAPE '\'` in some modes; the
/// same convention the search path uses (`ferris-search`'s `like_pattern`
/// before the crate was retired).
pub(super) fn escape_like_pattern(value: &str) -> String {
    value
        .replace('!', "!!")
        .replace('%', "!%")
        .replace('_', "!_")
}

/// Lowercase hex SHA-256 of `bytes`.
///
/// The digest is written by hand because `sha2`'s output type does not promise
/// a `LowerHex` impl across versions; the loop avoids adding a `hex` dependency
/// for sixteen bytes.
pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing into a `String` cannot fail; the macro returns `Result` only
        // because `fmt::Write` is generic over other sinks.
        let _ = write!(out, "{byte:02x}");
    }
    out
}
