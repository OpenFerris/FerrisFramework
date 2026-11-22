//! Table definitions and the shared column constants.
//!
//! Kept in their own fragment so `init` (DDL) and the query methods cannot drift
//! about which table or index a column belongs to.

use ferris_db::sql::Column;

pub const TAB_JOBS: &str = "jobs";
pub const TAB_JOB_WORKERS: &str = "job_workers";
/// One row per `depends_on` edge, so the claim can test "all my dependencies
/// succeeded" in SQL instead of decoding every candidate's JSON in Rust.
pub const TAB_JOB_DEPENDENCIES: &str = "job_dependencies";

pub(super) fn jobs_table() -> Vec<Column> {
    vec![
        Column::new("id", "VARCHAR(255)").not_null().primary_key(),
        Column::new("name", "VARCHAR(255)").not_null(),
        Column::new("queue", "VARCHAR(255)").not_null(),
        Column::new("payload", "TEXT").not_null(),
        Column::new("priority", "INTEGER").not_null().default("0"),
        Column::new("status", "VARCHAR(32)")
            .not_null()
            .default("'pending'"),
        Column::new("retries", "INTEGER").not_null().default("0"),
        Column::new("max_retries", "INTEGER")
            .not_null()
            .default("3"),
        Column::new("timeout_secs", "BIGINT")
            .not_null()
            .default("300"),
        Column::new("scheduled_at", "BIGINT"),
        Column::new("started_at", "BIGINT"),
        Column::new("completed_at", "BIGINT"),
        Column::new("depends_on", "TEXT"),
        Column::new("tags", "TEXT"),
        Column::new("metadata", "TEXT"),
        Column::new("created_at", "BIGINT").not_null(),
        Column::new("updated_at", "BIGINT").not_null(),
        Column::new("worker_id", "VARCHAR(255)"),
        Column::new("result", "TEXT"),
    ]
}

pub(super) fn job_workers_table() -> Vec<Column> {
    vec![
        Column::new("worker_id", "VARCHAR(255)")
            .not_null()
            .primary_key(),
        Column::new("status", "VARCHAR(32)").not_null(),
        Column::new("queues", "TEXT").not_null(),
        Column::new("current_jobs", "INTEGER")
            .not_null()
            .default("0"),
        Column::new("started_at", "BIGINT").not_null(),
        Column::new("last_heartbeat", "BIGINT").not_null(),
    ]
}

/// The normalised form of `jobs.depends_on`.
///
/// `depends_on` is a JSON array in the payload/column, which no portable SQL
/// can test membership against (the three backends disagree: `json_each`,
/// `jsonb_array_elements`, `JSON_CONTAINS`). Writing each edge as a row gives
/// the claim one portable `NOT EXISTS` subquery instead of three dialects'
/// worth of JSON functions. The composite key also makes the edges unique.
///
/// The edges are written by `JobStore::persist`. A job enqueued before this
/// table existed has no edges and is therefore not retroactively gated; it has
/// to be re-enqueued (or its row rewritten) to gain them.
pub(super) fn job_dependencies_table() -> Vec<Column> {
    vec![
        Column::new("job_id", "VARCHAR(255)")
            .not_null()
            .primary_key(),
        Column::new("depends_on_id", "VARCHAR(255)")
            .not_null()
            .primary_key(),
    ]
}
